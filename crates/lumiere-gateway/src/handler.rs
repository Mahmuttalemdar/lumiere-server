use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use lumiere_auth::jwt;
use lumiere_models::snowflake::Snowflake;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::mpsc;

use crate::{
    protocol::{GatewayMessage, IdentifyPayload, OpCode, ResumePayload},
    session::{GatewaySession, SessionManager},
};

/// Heartbeat interval in ms (≈41.25 seconds)
const HEARTBEAT_INTERVAL_MS: u64 = 41250;
/// Max time without heartbeat before closing (heartbeat_interval * 1.5)
const HEARTBEAT_TIMEOUT_MS: u64 = 61875;
/// Rate limit: max commands per 60 seconds
const MAX_COMMANDS_PER_MINUTE: u32 = 120;
/// Session buffer TTL in Redis (5 minutes)
const SESSION_BUFFER_TTL: i64 = 300;
/// Bounded channel capacity for outbound gateway messages
const OUTBOUND_CHANNEL_CAPACITY: usize = 1024;

/// Shared state needed by the gateway handler
pub struct GatewayState {
    pub session_manager: SessionManager,
    pub nats: lumiere_nats::NatsService,
    pub redis: redis::aio::ConnectionManager,
    pub jwt_secret: String,
    pub db: lumiere_db::Database,
}

/// WebSocket upgrade handler
pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_connection(socket, state))
}

async fn handle_connection(socket: WebSocket, state: Arc<GatewayState>) {
    let (mut ws_sink, mut ws_stream) = socket.split();

    // Send Hello
    let hello = GatewayMessage::hello(HEARTBEAT_INTERVAL_MS);
    if let Ok(text) = serde_json::to_string(&hello) {
        if ws_sink.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }

    // Create bounded outbound channel to apply backpressure
    let (tx, mut rx) = mpsc::channel::<GatewayMessage>(OUTBOUND_CHANNEL_CAPACITY);

    // Spawn sender task
    let mut sender_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(text) = serde_json::to_string(&msg) {
                if ws_sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Connection state
    let mut session: Option<Arc<GatewaySession>> = None;
    let mut nats_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut command_count = 0u32;
    let mut command_window_start = std::time::Instant::now();

    // Process incoming messages with heartbeat timeout
    loop {
        let msg = tokio::time::timeout(
            std::time::Duration::from_millis(HEARTBEAT_TIMEOUT_MS),
            ws_stream.next(),
        )
        .await;
        let msg = match msg {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(_))) | Ok(None) => break, // stream error or closed
            Err(_) => {
                // timeout — no heartbeat received
                tracing::warn!("Heartbeat timeout");
                break;
            }
        };
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            Message::Ping(_) => continue,
            _ => continue,
        };

        // Rate limiting
        if command_window_start.elapsed() > std::time::Duration::from_secs(60) {
            command_count = 0;
            command_window_start = std::time::Instant::now();
        }
        command_count += 1;
        if command_count > MAX_COMMANDS_PER_MINUTE {
            tracing::warn!("Gateway rate limit exceeded");
            break;
        }

        let gateway_msg: GatewayMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(_) => {
                let _ = tx.try_send(GatewayMessage::invalid_session(false));
                break;
            }
        };

        match gateway_msg.op {
            OpCode::Identify => {
                if session.is_some() {
                    // Already identified
                    break;
                }

                let payload: IdentifyPayload =
                    match gateway_msg.d.and_then(|d| serde_json::from_value(d).ok()) {
                        Some(p) => p,
                        None => {
                            let _ = tx.try_send(GatewayMessage::invalid_session(false));
                            break;
                        }
                    };

                match handle_identify(&state, &payload, tx.clone()).await {
                    Ok((s, handles)) => {
                        session = Some(s);
                        nats_handles = handles;
                    }
                    Err(_) => {
                        let _ = tx.try_send(GatewayMessage::invalid_session(false));
                        break;
                    }
                }
            }

            OpCode::Heartbeat => {
                if let Some(ref s) = session {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    s.last_heartbeat.store(now, Ordering::Relaxed);

                    // Refresh Redis presence TTL
                    let mut conn = state.redis.clone();
                    let key = format!("presence:{}", s.user_id);
                    let _: Result<(), _> = redis::cmd("EXPIRE")
                        .arg(&key)
                        .arg(300i64)
                        .query_async(&mut conn)
                        .await;
                }
                let _ = tx.try_send(GatewayMessage::heartbeat_ack());
            }

            OpCode::Resume => {
                let payload: ResumePayload =
                    match gateway_msg.d.and_then(|d| serde_json::from_value(d).ok()) {
                        Some(p) => p,
                        None => {
                            let _ = tx.try_send(GatewayMessage::invalid_session(false));
                            break;
                        }
                    };

                match handle_resume(&state, &payload, tx.clone()).await {
                    Ok((s, handles)) => {
                        // Cancel old NATS subscriptions before replacing
                        for h in nats_handles.drain(..) {
                            h.abort();
                        }
                        session = Some(s);
                        nats_handles = handles;
                    }
                    Err(_) => {
                        let _ = tx.try_send(GatewayMessage::invalid_session(false));
                        break;
                    }
                }
            }

            OpCode::PresenceUpdate => {
                if let Some(ref s) = session {
                    if let Some(data) = gateway_msg.d {
                        handle_presence_update(&state, s, data).await;
                    }
                }
            }

            _ => {
                // Unknown or server-only opcode
            }
        }
    }

    // Cleanup: abort all NATS listener tasks
    for h in nats_handles {
        h.abort();
    }

    // Drop the sender so the sender_task can flush remaining messages (e.g., InvalidSession)
    // and then exit cleanly when its rx.recv() returns None.
    drop(tx);
    // Give the sender task a moment to flush any queued messages before we continue cleanup.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), &mut sender_task).await;
    sender_task.abort();

    // Cleanup: remove session
    if let Some(ref s) = session {
        // Store session buffer in Redis for resume
        let session_id = s.session_id.clone();
        let user_id = s.user_id;
        let sequence = s.sequence.load(Ordering::Relaxed);

        let mut conn = state.redis.clone();
        let session_data = serde_json::json!({
            "user_id": user_id.to_string(),
            "sequence": sequence,
        });
        let _: Result<(), _> = redis::cmd("SET")
            .arg(format!("gateway_session:{}", session_id))
            .arg(session_data.to_string())
            .arg("EX")
            .arg(SESSION_BUFFER_TTL)
            .query_async(&mut conn)
            .await;

        state.session_manager.remove(&session_id);

        // Broadcast offline presence after a short delay (allow for resume)
        let nats = state.nats.clone();
        let uid = user_id;
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            // Check if user has reconnected before broadcasting offline
            if !state_clone
                .session_manager
                .user_sessions
                .contains_key(&uid.value())
            {
                let event = serde_json::json!({
                    "type": "PRESENCE_UPDATE",
                    "user_id": uid,
                    "status": "offline",
                });
                let _ = nats
                    .publish(&format!("user.{}.presence", uid), &event)
                    .await;
            }
        });
    }
}

async fn handle_identify(
    state: &GatewayState,
    payload: &IdentifyPayload,
    tx: mpsc::Sender<GatewayMessage>,
) -> Result<(Arc<GatewaySession>, Vec<tokio::task::JoinHandle<()>>), anyhow::Error> {
    // Validate token
    let claims = jwt::verify_token(&payload.token, &state.jwt_secret)?;
    if claims.token_type != jwt::TokenType::Access {
        anyhow::bail!("Not an access token");
    }

    let user_id: Snowflake = claims.sub.parse()?;

    // Load user data
    let user_row = sqlx::query_as::<_, (i64, String, i16, String, Option<String>, Option<String>, Option<String>, String, i64, i16, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, username, discriminator, email, avatar, banner, bio, locale, flags, premium_type, is_bot, created_at \
         FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| anyhow::anyhow!("User not found"))?;

    // Load server list
    let servers = sqlx::query_as::<_, (i64, String, Option<String>, i64, i32)>(
        "SELECT s.id, s.name, s.icon, s.owner_id, s.member_count \
         FROM servers s \
         JOIN server_members sm ON sm.server_id = s.id \
         WHERE sm.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.db.pg)
    .await?;

    // Load DM channels
    let dms = sqlx::query_as::<_, (i64, i16)>(
        "SELECT c.id, c.type FROM channels c \
         JOIN dm_recipients dr ON dr.channel_id = c.id \
         WHERE dr.user_id = $1 AND c.type IN (1, 3)",
    )
    .bind(user_id)
    .fetch_all(&state.db.pg)
    .await?;

    // Create session
    let session_id = nanoid::nanoid!(32);
    let session = Arc::new(GatewaySession {
        session_id: session_id.clone(),
        user_id,
        sequence: AtomicU64::new(0),
        sender: tx.clone(),
        last_heartbeat: AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        ),
        connected_at: std::time::Instant::now(),
    });

    state.session_manager.register(Arc::clone(&session));

    // Build Ready event
    let ready_data = serde_json::json!({
        "user": {
            "id": user_row.0.to_string(),
            "username": user_row.1,
            "discriminator": user_row.2,
            "avatar": user_row.4,
            "banner": user_row.5,
            "bio": user_row.6,
        },
        "servers": servers.iter().map(|s| serde_json::json!({
            "id": s.0.to_string(),
            "name": s.1,
            "icon": s.2,
            "owner_id": s.3.to_string(),
            "member_count": s.4,
        })).collect::<Vec<_>>(),
        "private_channels": dms.iter().map(|d| serde_json::json!({
            "id": d.0.to_string(),
            "type": d.1,
        })).collect::<Vec<_>>(),
        "session_id": session_id,
    });

    let seq = SessionManager::next_sequence(&session);
    let _ = tx.try_send(GatewayMessage::dispatch("READY", seq, ready_data));

    // Subscribe to NATS subjects and forward events
    let handles = spawn_nats_listener(state, &session, &servers, &dms).await;

    // Broadcast online presence to user's own subject and all their servers
    let presence_event = serde_json::json!({
        "type": "PRESENCE_UPDATE",
        "user_id": user_id,
        "status": "online",
    });
    let _ = state
        .nats
        .publish(&format!("user.{}.presence", user_id), &presence_event)
        .await;
    for (server_id, _, _, _, _) in &servers {
        let _ = state
            .nats
            .publish(&format!("server.{}.events", server_id), &presence_event)
            .await;
    }

    Ok((session, handles))
}

async fn handle_resume(
    state: &GatewayState,
    payload: &ResumePayload,
    tx: mpsc::Sender<GatewayMessage>,
) -> Result<(Arc<GatewaySession>, Vec<tokio::task::JoinHandle<()>>), anyhow::Error> {
    // Validate token
    let claims = jwt::verify_token(&payload.token, &state.jwt_secret)?;
    if claims.token_type != jwt::TokenType::Access {
        anyhow::bail!("Not an access token");
    }
    let user_id: Snowflake = claims.sub.parse()?;

    // Check session exists in Redis
    let mut conn = state.redis.clone();
    let session_data: Option<String> = redis::cmd("GET")
        .arg(format!("gateway_session:{}", payload.session_id))
        .query_async(&mut conn)
        .await?;

    let session_data = session_data.ok_or_else(|| anyhow::anyhow!("Session expired"))?;
    let session_json: serde_json::Value = serde_json::from_str(&session_data)?;

    let stored_user_id: String = session_json["user_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid session data"))?
        .to_string();

    if stored_user_id != user_id.to_string() {
        anyhow::bail!("Session user mismatch");
    }

    // Delete session from Redis (consumed)
    let _: () = redis::cmd("DEL")
        .arg(format!("gateway_session:{}", payload.session_id))
        .query_async(&mut conn)
        .await?;

    // Create new session with same ID
    let session = Arc::new(GatewaySession {
        session_id: payload.session_id.clone(),
        user_id,
        sequence: AtomicU64::new(payload.sequence),
        sender: tx.clone(),
        last_heartbeat: AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        ),
        connected_at: std::time::Instant::now(),
    });

    state.session_manager.register(Arc::clone(&session));

    // Replay missed events before sending RESUMED
    let mut replay_conn = state.redis.clone();
    let replayed =
        replay_events(&mut replay_conn, &payload.session_id, payload.sequence, &tx).await;
    tracing::debug!(
        session_id = %payload.session_id,
        last_sequence = payload.sequence,
        replayed,
        "Replayed missed events on resume"
    );

    // Send RESUMED event
    let seq = SessionManager::next_sequence(&session);
    let _ = tx.try_send(GatewayMessage::dispatch(
        "RESUMED",
        seq,
        serde_json::json!({}),
    ));

    // Re-subscribe to NATS
    let servers = sqlx::query_as::<_, (i64, String, Option<String>, i64, i32)>(
        "SELECT s.id, s.name, s.icon, s.owner_id, s.member_count \
         FROM servers s JOIN server_members sm ON sm.server_id = s.id WHERE sm.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.db.pg)
    .await?;

    let dms = sqlx::query_as::<_, (i64, i16)>(
        "SELECT c.id, c.type FROM channels c \
         JOIN dm_recipients dr ON dr.channel_id = c.id WHERE dr.user_id = $1 AND c.type IN (1, 3)",
    )
    .bind(user_id)
    .fetch_all(&state.db.pg)
    .await?;

    let handles = spawn_nats_listener(state, &session, &servers, &dms).await;

    Ok((session, handles))
}

async fn handle_presence_update(
    state: &GatewayState,
    session: &GatewaySession,
    data: serde_json::Value,
) {
    let status = data["status"].as_str().unwrap_or("online");
    let valid = ["online", "idle", "dnd", "invisible"];
    if !valid.contains(&status) {
        return;
    }

    // Update Redis presence
    let mut conn = state.redis.clone();
    let key = format!("presence:{}", session.user_id);
    let presence = serde_json::json!({
        "status": status,
        "custom_status": data.get("custom_status"),
        "last_active": chrono::Utc::now().to_rfc3339(),
    });
    let _: Result<(), _> = redis::cmd("SET")
        .arg(&key)
        .arg(presence.to_string())
        .arg("EX")
        .arg(300i64)
        .query_async(&mut conn)
        .await;

    // Broadcast if not invisible
    if status != "invisible" {
        let event = serde_json::json!({
            "type": "PRESENCE_UPDATE",
            "user_id": session.user_id,
            "status": status,
            "custom_status": data.get("custom_status"),
        });
        let _ = state
            .nats
            .publish(&format!("user.{}.presence", session.user_id), &event)
            .await;

        // Also broadcast to all servers the user is in
        let servers =
            sqlx::query_scalar::<_, i64>("SELECT server_id FROM server_members WHERE user_id = $1")
                .bind(session.user_id)
                .fetch_all(&state.db.pg)
                .await
                .unwrap_or_default();
        for server_id in servers {
            let _ = state
                .nats
                .publish(&format!("server.{}.events", server_id), &event)
                .await;
        }
    }
}

/// Buffer a dispatch event in Redis for session resume/replay.
/// Only buffers Dispatch events. Keeps the last 1000 events with a 5-minute TTL.
async fn buffer_event(
    redis: &mut redis::aio::ConnectionManager,
    session_id: &str,
    event: &GatewayMessage,
) {
    if event.op != OpCode::Dispatch {
        return;
    }
    let key = format!("gateway_events:{}", session_id);
    if let Ok(json) = serde_json::to_string(event) {
        let _: Result<(), _> = redis::pipe()
            .cmd("RPUSH")
            .arg(&key)
            .arg(&json)
            .cmd("LTRIM")
            .arg(&key)
            .arg(-1000i64)
            .arg(-1i64)
            .cmd("EXPIRE")
            .arg(&key)
            .arg(SESSION_BUFFER_TTL)
            .query_async(redis)
            .await;
    }
}

/// Replay missed events from Redis on session resume.
/// Returns the number of events replayed.
async fn replay_events(
    redis: &mut redis::aio::ConnectionManager,
    session_id: &str,
    last_sequence: u64,
    tx: &mpsc::Sender<GatewayMessage>,
) -> u64 {
    let key = format!("gateway_events:{}", session_id);
    let events: Vec<String> = redis::cmd("LRANGE")
        .arg(&key)
        .arg(0i64)
        .arg(-1i64)
        .query_async(redis)
        .await
        .unwrap_or_default();

    let mut replayed = 0u64;
    for json in events {
        if let Ok(event) = serde_json::from_str::<GatewayMessage>(&json) {
            if let Some(seq) = event.s {
                if seq > last_sequence {
                    let _ = tx.try_send(event);
                    replayed += 1;
                }
            }
        }
    }

    // Clean up the buffer after replay
    let _: Result<(), _> = redis::cmd("DEL").arg(&key).query_async(redis).await;
    replayed
}

/// Spawn NATS subscription listeners that forward events to the WebSocket
async fn spawn_nats_listener(
    state: &GatewayState,
    session: &Arc<GatewaySession>,
    servers: &[(i64, String, Option<String>, i64, i32)],
    dms: &[(i64, i16)],
) -> Vec<tokio::task::JoinHandle<()>> {
    let user_id = session.user_id;
    let mut handles = Vec::new();

    // Subscribe to user events
    handles.push(subscribe_and_forward(
        &state.nats.client,
        &format!("user.{}.>", user_id),
        session,
        state.redis.clone(),
    ));

    // Subscribe to server events
    for (server_id, _, _, _, _) in servers {
        handles.push(subscribe_and_forward(
            &state.nats.client,
            &format!("server.{}.>", server_id),
            session,
            state.redis.clone(),
        ));
    }

    // Subscribe to DM channel events
    for (channel_id, _) in dms {
        handles.push(subscribe_and_forward(
            &state.nats.client,
            &format!("channel.{}.>", channel_id),
            session,
            state.redis.clone(),
        ));
    }

    handles
}

fn subscribe_and_forward(
    client: &async_nats::Client,
    subject: &str,
    session: &Arc<GatewaySession>,
    redis: redis::aio::ConnectionManager,
) -> tokio::task::JoinHandle<()> {
    let client = client.clone();
    let subject = subject.to_string();
    let session = Arc::clone(session);
    let mut redis = redis;

    tokio::spawn(async move {
        let mut subscriber = match client.subscribe(subject).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to subscribe: {}", e);
                return;
            }
        };

        while let Some(msg) = subscriber.next().await {
            let data: serde_json::Value = match serde_json::from_slice(&msg.payload) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let event_type = data["type"].as_str().unwrap_or("UNKNOWN").to_string();
            let seq = SessionManager::next_sequence(&session);
            let dispatch = GatewayMessage::dispatch(&event_type, seq, data);

            // Buffer the event in Redis for potential session resume
            buffer_event(&mut redis, &session.session_id, &dispatch).await;

            if session.sender.try_send(dispatch).is_err() {
                break; // Connection closed or channel full
            }
        }
    })
}
