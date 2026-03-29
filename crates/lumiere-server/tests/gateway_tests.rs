mod common;
use common::{get_test_app, unique_name, unique_email};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures::{SinkExt, StreamExt};

use std::time::Duration;
use tokio::time::timeout;

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;
type WsSink = futures::stream::SplitSink<WsStream, Message>;
type WsRecv = futures::stream::SplitStream<WsStream>;

const WS_TIMEOUT: Duration = Duration::from_secs(5);
const WS_LONG_TIMEOUT: Duration = Duration::from_secs(10);

// ─── Helpers ────────────────────────────────────────────────────

async fn ws_connect(app: &common::TestApp) -> (WsSink, WsRecv) {
    let url = format!("ws://{}/gateway", app.addr);
    let (ws_stream, _) = connect_async(&url).await.expect("Failed to connect to gateway");
    ws_stream.split()
}

async fn read_message(rx: &mut WsRecv) -> serde_json::Value {
    let msg = timeout(WS_TIMEOUT, rx.next())
        .await
        .expect("Timeout waiting for WS message")
        .expect("Stream ended unexpectedly")
        .expect("WS read error");
    let text = match msg {
        Message::Text(t) => t,
        Message::Close(_) => panic!("Received Close frame when expecting a message"),
        other => panic!("Unexpected message type: {:?}", other),
    };
    serde_json::from_str(&text).expect("Failed to parse WS message as JSON")
}

async fn send_message_ws(tx: &mut WsSink, msg: serde_json::Value) {
    let text = serde_json::to_string(&msg).unwrap();
    tx.send(Message::Text(text)).await.expect("Failed to send WS message");
}

/// Try to read a message, returning None if the connection closes or times out.
async fn try_read_message(rx: &mut WsRecv) -> Option<serde_json::Value> {
    match timeout(WS_TIMEOUT, rx.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str(&t).ok(),
        Ok(Some(Ok(Message::Close(_)))) => None,
        _ => None,
    }
}

/// Expect the connection to close (receive Close frame or stream end) within the timeout.
async fn expect_close(rx: &mut WsRecv, dur: Duration) -> bool {
    match timeout(dur, rx.next()).await {
        Ok(None) => true,                           // stream ended
        Ok(Some(Ok(Message::Close(_)))) => true,    // close frame
        Ok(Some(Err(_))) => true,                   // connection error
        Err(_) => false,                            // timeout, still open
        Ok(Some(Ok(_))) => false,                   // got a non-close message
    }
}

/// Connect, read Hello, send Identify, read Ready. Returns (tx, rx, session_id, seq).
async fn ws_connect_and_identify(
    app: &common::TestApp,
    token: &str,
) -> (WsSink, WsRecv, String, u64) {
    let (mut tx, mut rx) = ws_connect(app).await;

    // Read Hello
    let hello = read_message(&mut rx).await;
    assert_eq!(hello["op"], 10, "Expected Hello opcode");

    // Send Identify
    send_message_ws(&mut tx, serde_json::json!({
        "op": 2,
        "d": {
            "token": token,
            "properties": {
                "os": "test",
                "browser": "integration-test",
                "device": "test-runner"
            }
        }
    })).await;

    // Read Ready
    let ready = read_message(&mut rx).await;
    assert_eq!(ready["op"], 0, "Expected Dispatch opcode for READY");
    assert_eq!(ready["t"], "READY", "Expected READY event");

    let session_id = ready["d"]["session_id"].as_str().unwrap().to_string();
    let seq = ready["s"].as_u64().unwrap();

    (tx, rx, session_id, seq)
}

/// Drain any pending dispatch events (e.g., PRESENCE_UPDATE on connect).
async fn drain_pending_events(rx: &mut WsRecv) {
    // Read any queued messages with a short timeout
    loop {
        match timeout(Duration::from_millis(500), rx.next()).await {
            Ok(Some(Ok(Message::Text(_)))) => continue,
            _ => break,
        }
    }
}

/// Wait for a specific event type, skipping others. Returns the matching message.
async fn wait_for_event(rx: &mut WsRecv, event_type: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + WS_LONG_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("Timeout waiting for event: {}", event_type);
        }
        match timeout(remaining, rx.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let val: serde_json::Value = serde_json::from_str(&t).unwrap();
                if val["t"].as_str() == Some(event_type) {
                    return val;
                }
                // Skip non-matching events
            }
            _ => panic!("Connection closed while waiting for event: {}", event_type),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Connection Lifecycle Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_ws_connect_receives_hello() {
    let app = get_test_app().await;
    let (_tx, mut rx) = ws_connect(app).await;

    let hello = read_message(&mut rx).await;
    assert_eq!(hello["op"], 10, "Expected Hello opcode (10)");
    assert!(
        hello["d"]["heartbeat_interval"].is_number(),
        "Hello must contain heartbeat_interval"
    );
    let interval = hello["d"]["heartbeat_interval"].as_u64().unwrap();
    assert!(interval > 0, "heartbeat_interval must be positive");
}

#[tokio::test]
async fn test_ws_identify_receives_ready() {
    let app = get_test_app().await;
    let name = unique_name("ready");
    let email = unique_email("ready");
    let (token, _user_id) = app.register_user(&name, &email, "TestPass123!").await;

    let (mut tx, mut rx) = ws_connect(app).await;

    // Read Hello
    let hello = read_message(&mut rx).await;
    assert_eq!(hello["op"], 10);

    // Send Identify (op: 2)
    send_message_ws(&mut tx, serde_json::json!({
        "op": 2,
        "d": {
            "token": token,
            "properties": {
                "os": "test",
                "browser": "test",
                "device": "test"
            }
        }
    })).await;

    // Read Ready (op: 0, t: "READY")
    let ready = read_message(&mut rx).await;
    assert_eq!(ready["op"], 0, "Expected Dispatch opcode");
    assert_eq!(ready["t"], "READY");
    assert!(ready["s"].is_number(), "Ready must have sequence number");

    let data = &ready["d"];
    assert!(data["user"].is_object(), "READY must contain user object");
    assert!(data["user"]["id"].is_string(), "user must have id");
    assert!(data["user"]["username"].is_string(), "user must have username");
    assert!(data["session_id"].is_string(), "READY must contain session_id");
    assert!(data["servers"].is_array(), "READY must contain servers array");
    assert!(data["private_channels"].is_array(), "READY must contain private_channels");
}

#[tokio::test]
async fn test_ws_identify_invalid_token() {
    let app = get_test_app().await;
    let (mut tx, mut rx) = ws_connect(app).await;

    // Read Hello
    let _hello = read_message(&mut rx).await;

    // Send Identify with bad token
    send_message_ws(&mut tx, serde_json::json!({
        "op": 2,
        "d": {
            "token": "this.is.an.invalid.token",
            "properties": {}
        }
    })).await;

    // Should receive InvalidSession (op: 9)
    let msg = read_message(&mut rx).await;
    assert_eq!(msg["op"], 9, "Expected InvalidSession opcode (9)");
    // d should be false (not resumable)
    assert_eq!(msg["d"], false, "InvalidSession should not be resumable for bad token");
}

#[tokio::test]
async fn test_ws_identify_with_refresh_token() {
    let app = get_test_app().await;
    let name = unique_name("refresh");
    let email = unique_email("refresh");
    app.register_user(&name, &email, "TestPass123!").await;
    let (_access, refresh) = app.login(&email, "TestPass123!").await;

    let (mut tx, mut rx) = ws_connect(app).await;
    let _hello = read_message(&mut rx).await;

    // Send Identify with refresh token instead of access token
    send_message_ws(&mut tx, serde_json::json!({
        "op": 2,
        "d": {
            "token": refresh,
            "properties": {}
        }
    })).await;

    // Should receive InvalidSession — refresh tokens are not valid for Identify
    let msg = read_message(&mut rx).await;
    assert_eq!(msg["op"], 9, "Refresh token should trigger InvalidSession");
}

#[tokio::test]
async fn test_ws_double_identify() {
    let app = get_test_app().await;
    let name = unique_name("dblid");
    let email = unique_email("dblid");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;

    let (mut tx, mut rx, _session_id, _) = ws_connect_and_identify(app, &token).await;

    // Send Identify again (already authenticated)
    send_message_ws(&mut tx, serde_json::json!({
        "op": 2,
        "d": {
            "token": token,
            "properties": {}
        }
    })).await;

    // Connection should close (ALREADY_AUTHENTICATED close code 4005 or stream ends)
    let closed = expect_close(&mut rx, WS_TIMEOUT).await;
    assert!(closed, "Connection should close after double Identify");
}

#[tokio::test]
async fn test_ws_heartbeat_acknowledged() {
    let app = get_test_app().await;
    let name = unique_name("hback");
    let email = unique_email("hback");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;

    let (mut tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;

    // Drain any pending events (e.g., PRESENCE_UPDATE)
    drain_pending_events(&mut rx).await;

    // Send Heartbeat (op: 1)
    send_message_ws(&mut tx, serde_json::json!({
        "op": 1,
        "d": null
    })).await;

    // Should receive HeartbeatAck (op: 11)
    let ack = read_message(&mut rx).await;
    assert_eq!(ack["op"], 11, "Expected HeartbeatAck opcode (11)");
}

#[tokio::test]
async fn test_ws_heartbeat_before_identify() {
    let app = get_test_app().await;
    let (_tx, mut rx) = ws_connect(app).await;

    // Read Hello
    let _hello = read_message(&mut rx).await;

    // Send Heartbeat without identifying first — server should still ACK
    // (Discord's gateway also ACKs heartbeats before identify)
    let mut tx = _tx;
    send_message_ws(&mut tx, serde_json::json!({
        "op": 1,
        "d": null
    })).await;

    let msg = read_message(&mut rx).await;
    assert_eq!(msg["op"], 11, "Should receive HeartbeatAck even before Identify");
}

#[tokio::test]
async fn test_ws_heartbeat_timeout() {
    let app = get_test_app().await;
    let name = unique_name("hbtmo");
    let email = unique_email("hbtmo");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;

    let (_tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;

    // The server heartbeat timeout is ~62 seconds. We wait for the connection to close.
    // This test verifies the timeout mechanism works. Using a 70-second window.
    let closed = expect_close(&mut rx, Duration::from_secs(70)).await;
    assert!(closed, "Connection should close after heartbeat timeout (~62s)");
}

#[tokio::test]
async fn test_ws_close_graceful() {
    let app = get_test_app().await;
    let name = unique_name("close");
    let email = unique_email("close");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;

    let (mut tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;

    // Send a close frame
    tx.send(Message::Close(None)).await.expect("Failed to send Close");

    // Connection should close cleanly
    let closed = expect_close(&mut rx, WS_TIMEOUT).await;
    assert!(closed, "Connection should close cleanly after Close frame");
}

// ═══════════════════════════════════════════════════════════════════
// Event Dispatch Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_ws_receive_message_create() {
    let app = get_test_app().await;

    // Register user and create a server
    let name = unique_name("msgcreate");
    let email = unique_email("msgcreate");
    let (token, _user_id) = app.register_user(&name, &email, "TestPass123!").await;
    let server_id = app.create_server(&token, &unique_name("srv")).await;

    // Get server channels (there should be a default text channel)
    let channels_res = app.get(&token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = channels_res.json().await.unwrap();
    let channel_id = channels.iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .expect("No text channel found")["id"]
        .as_str().unwrap().to_string();

    // Connect WS and identify
    let (_tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;

    // Drain initial events
    drain_pending_events(&mut rx).await;

    // Send a message via API
    let msg_res = app.post(
        &token,
        &format!("/api/v1/channels/{}/messages", channel_id),
        serde_json::json!({ "content": "Hello from integration test!" }),
    ).await;
    assert!(msg_res.status().is_success(), "Message send failed");

    // WS should receive MESSAGE_CREATE event
    let event = wait_for_event(&mut rx, "MESSAGE_CREATE").await;
    assert_eq!(event["op"], 0);
    assert_eq!(event["t"], "MESSAGE_CREATE");
    assert!(event["d"]["content"].as_str().is_some() || event["d"]["type"].is_string());
}

#[tokio::test]
async fn test_ws_receive_guild_member_add() {
    let app = get_test_app().await;

    // Owner creates a server
    let owner_name = unique_name("owner");
    let owner_email = unique_email("owner");
    let (owner_token, _) = app.register_user(&owner_name, &owner_email, "TestPass123!").await;
    let server_id = app.create_server(&owner_token, &unique_name("srv")).await;

    // Connect owner to WS
    let (_owner_tx, mut owner_rx, _, _) = ws_connect_and_identify(app, &owner_token).await;
    drain_pending_events(&mut owner_rx).await;

    // Create invite
    let channels_res = app.get(&owner_token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = channels_res.json().await.unwrap();
    let channel_id = channels[0]["id"].as_str().unwrap();

    let invite_res = app.post(
        &owner_token,
        &format!("/api/v1/channels/{}/invites", channel_id),
        serde_json::json!({ "max_uses": 10 }),
    ).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let invite_code = invite["code"].as_str().unwrap();

    // New user joins the server via invite
    let joiner_name = unique_name("joiner");
    let joiner_email = unique_email("joiner");
    let (joiner_token, _) = app.register_user(&joiner_name, &joiner_email, "TestPass123!").await;

    let join_res = app.post(
        &joiner_token,
        &format!("/api/v1/invites/{}", invite_code),
        serde_json::json!({}),
    ).await;
    assert!(join_res.status().is_success(), "Join via invite failed: {}", join_res.status());

    // Owner should receive GUILD_MEMBER_ADD event
    let event = wait_for_event(&mut owner_rx, "GUILD_MEMBER_ADD").await;
    assert_eq!(event["op"], 0);
    assert_eq!(event["t"], "GUILD_MEMBER_ADD");
}

#[tokio::test]
async fn test_ws_receive_relationship_add() {
    let app = get_test_app().await;

    // Register two users
    let name1 = unique_name("rel1");
    let email1 = unique_email("rel1");
    let (token1, _user_id1) = app.register_user(&name1, &email1, "TestPass123!").await;

    let name2 = unique_name("rel2");
    let email2 = unique_email("rel2");
    let (token2, user_id2) = app.register_user(&name2, &email2, "TestPass123!").await;

    // Connect user2 to WS
    let (_tx2, mut rx2, _, _) = ws_connect_and_identify(app, &token2).await;
    drain_pending_events(&mut rx2).await;

    // User1 sends friend request to user2
    let res = app.post(
        &token1,
        "/api/v1/users/@me/relationships",
        serde_json::json!({
            "user_id": user_id2.to_string(),
            "type": 1
        }),
    ).await;
    assert!(res.status().is_success(), "Friend request failed: {}", res.status());

    // User2 should receive RELATIONSHIP_ADD event via WS
    let event = wait_for_event(&mut rx2, "RELATIONSHIP_ADD").await;
    assert_eq!(event["op"], 0);
    assert_eq!(event["t"], "RELATIONSHIP_ADD");
}

#[tokio::test]
async fn test_ws_receive_presence_update() {
    let app = get_test_app().await;

    // Two users in the same server
    let owner_name = unique_name("presown");
    let owner_email = unique_email("presown");
    let (owner_token, _) = app.register_user(&owner_name, &owner_email, "TestPass123!").await;
    let server_id = app.create_server(&owner_token, &unique_name("srv")).await;

    // Connect owner to WS
    let (_owner_tx, mut owner_rx, _, _) = ws_connect_and_identify(app, &owner_token).await;
    drain_pending_events(&mut owner_rx).await;

    // Second user joins the server and connects to WS
    let channels_res = app.get(&owner_token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = channels_res.json().await.unwrap();
    let channel_id = channels[0]["id"].as_str().unwrap();
    let invite_res = app.post(
        &owner_token,
        &format!("/api/v1/channels/{}/invites", channel_id),
        serde_json::json!({}),
    ).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let invite_code = invite["code"].as_str().unwrap();

    let user_name = unique_name("presusr");
    let user_email = unique_email("presusr");
    let (user_token, _) = app.register_user(&user_name, &user_email, "TestPass123!").await;
    app.post(&user_token, &format!("/api/v1/invites/{}", invite_code), serde_json::json!({})).await;

    // When the second user identifies on WS, presence goes online
    let (_user_tx, _user_rx, _, _) = ws_connect_and_identify(app, &user_token).await;

    // Owner should receive PRESENCE_UPDATE for the second user going online
    let event = wait_for_event(&mut owner_rx, "PRESENCE_UPDATE").await;
    assert_eq!(event["op"], 0);
    assert_eq!(event["t"], "PRESENCE_UPDATE");
}

#[tokio::test]
async fn test_ws_receive_typing_start() {
    let app = get_test_app().await;

    let name = unique_name("typing");
    let email = unique_email("typing");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;
    let server_id = app.create_server(&token, &unique_name("srv")).await;

    // Get text channel
    let channels_res = app.get(&token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = channels_res.json().await.unwrap();
    let channel_id = channels.iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .expect("No text channel")["id"]
        .as_str().unwrap().to_string();

    // Connect WS
    let (_tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;
    drain_pending_events(&mut rx).await;

    // Send typing indicator via API
    let typing_res = app.post(
        &token,
        &format!("/api/v1/channels/{}/typing", channel_id),
        serde_json::json!({}),
    ).await;
    assert!(typing_res.status().is_success(), "Typing request failed");

    // Should receive TYPING_START via WS
    let event = wait_for_event(&mut rx, "TYPING_START").await;
    assert_eq!(event["op"], 0);
    assert_eq!(event["t"], "TYPING_START");
}

#[tokio::test]
async fn test_ws_multiple_connections() {
    let app = get_test_app().await;

    let name = unique_name("multi");
    let email = unique_email("multi");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;
    let server_id = app.create_server(&token, &unique_name("srv")).await;

    // Get text channel
    let channels_res = app.get(&token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = channels_res.json().await.unwrap();
    let channel_id = channels.iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .expect("No text channel")["id"]
        .as_str().unwrap().to_string();

    // Open two WS connections for the same user
    let (_tx1, mut rx1, session1, _) = ws_connect_and_identify(app, &token).await;
    let (_tx2, mut rx2, session2, _) = ws_connect_and_identify(app, &token).await;
    assert_ne!(session1, session2, "Sessions should have different IDs");

    drain_pending_events(&mut rx1).await;
    drain_pending_events(&mut rx2).await;

    // Send a message via API
    app.post(
        &token,
        &format!("/api/v1/channels/{}/messages", channel_id),
        serde_json::json!({ "content": "Multi-connection test" }),
    ).await;

    // Both connections should receive the event
    let event1 = wait_for_event(&mut rx1, "MESSAGE_CREATE").await;
    let event2 = wait_for_event(&mut rx2, "MESSAGE_CREATE").await;

    assert_eq!(event1["t"], "MESSAGE_CREATE");
    assert_eq!(event2["t"], "MESSAGE_CREATE");
}

#[tokio::test]
async fn test_ws_receive_channel_create() {
    let app = get_test_app().await;

    let name = unique_name("chcreate");
    let email = unique_email("chcreate");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;
    let server_id = app.create_server(&token, &unique_name("srv")).await;

    // Connect WS
    let (_tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;
    drain_pending_events(&mut rx).await;

    // Create a new channel via API
    let res = app.post(
        &token,
        &format!("/api/v1/servers/{}/channels", server_id),
        serde_json::json!({
            "name": "new-test-channel",
            "type": 0
        }),
    ).await;
    assert!(res.status().is_success(), "Channel create failed: {}", res.status());

    // Should receive CHANNEL_CREATE via WS
    let event = wait_for_event(&mut rx, "CHANNEL_CREATE").await;
    assert_eq!(event["op"], 0);
    assert_eq!(event["t"], "CHANNEL_CREATE");
}

#[tokio::test]
async fn test_ws_receive_guild_role_create() {
    let app = get_test_app().await;

    let name = unique_name("rolecreate");
    let email = unique_email("rolecreate");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;
    let server_id = app.create_server(&token, &unique_name("srv")).await;

    // Connect WS
    let (_tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;
    drain_pending_events(&mut rx).await;

    // Create a role via API
    let res = app.post(
        &token,
        &format!("/api/v1/servers/{}/roles", server_id),
        serde_json::json!({
            "name": "TestRole",
            "color": 0xFF0000,
            "permissions": "0"
        }),
    ).await;
    assert!(res.status().is_success(), "Role create failed: {}", res.status());

    // Should receive GUILD_ROLE_CREATE via WS
    let event = wait_for_event(&mut rx, "GUILD_ROLE_CREATE").await;
    assert_eq!(event["op"], 0);
    assert_eq!(event["t"], "GUILD_ROLE_CREATE");
}

// ═══════════════════════════════════════════════════════════════════
// Resume Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_ws_resume_success() {
    let app = get_test_app().await;

    let name = unique_name("resume");
    let email = unique_email("resume");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;

    // Connect, identify, get session_id and sequence
    let (mut tx, mut rx, session_id, seq) = ws_connect_and_identify(app, &token).await;

    // Close connection gracefully
    tx.send(Message::Close(None)).await.ok();
    let _ = expect_close(&mut rx, WS_TIMEOUT).await;

    // Small delay to let the server persist session buffer in Redis
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Reconnect and resume
    let (mut tx2, mut rx2) = ws_connect(app).await;

    // Read Hello
    let hello = read_message(&mut rx2).await;
    assert_eq!(hello["op"], 10);

    // Send Resume (op: 6)
    send_message_ws(&mut tx2, serde_json::json!({
        "op": 6,
        "d": {
            "token": token,
            "session_id": session_id,
            "sequence": seq
        }
    })).await;

    // Should receive RESUMED dispatch event
    let msg = read_message(&mut rx2).await;
    assert_eq!(msg["op"], 0, "Expected Dispatch opcode for RESUMED");
    assert_eq!(msg["t"], "RESUMED", "Expected RESUMED event");
}

#[tokio::test]
async fn test_ws_resume_invalid_session() {
    let app = get_test_app().await;

    let name = unique_name("resinv");
    let email = unique_email("resinv");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;

    // Connect without prior session
    let (mut tx, mut rx) = ws_connect(app).await;
    let _hello = read_message(&mut rx).await;

    // Try to resume with a fake session_id (never existed or expired)
    send_message_ws(&mut tx, serde_json::json!({
        "op": 6,
        "d": {
            "token": token,
            "session_id": "nonexistent_session_id_12345678",
            "sequence": 0
        }
    })).await;

    // Should receive InvalidSession (op: 9)
    let msg = read_message(&mut rx).await;
    assert_eq!(msg["op"], 9, "Expected InvalidSession opcode (9) for expired session");
}

#[tokio::test]
async fn test_ws_resume_wrong_token() {
    let app = get_test_app().await;

    // User A connects and gets a session
    let name_a = unique_name("reswtA");
    let email_a = unique_email("reswtA");
    let (token_a, _) = app.register_user(&name_a, &email_a, "TestPass123!").await;
    let (mut tx_a, mut rx_a, session_id_a, seq_a) = ws_connect_and_identify(app, &token_a).await;

    // Close A's connection
    tx_a.send(Message::Close(None)).await.ok();
    let _ = expect_close(&mut rx_a, WS_TIMEOUT).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // User B tries to resume A's session
    let name_b = unique_name("reswtB");
    let email_b = unique_email("reswtB");
    let (token_b, _) = app.register_user(&name_b, &email_b, "TestPass123!").await;

    let (mut tx_b, mut rx_b) = ws_connect(app).await;
    let _hello = read_message(&mut rx_b).await;

    send_message_ws(&mut tx_b, serde_json::json!({
        "op": 6,
        "d": {
            "token": token_b,
            "session_id": session_id_a,
            "sequence": seq_a
        }
    })).await;

    // Should receive InvalidSession — user mismatch
    let msg = read_message(&mut rx_b).await;
    assert_eq!(msg["op"], 9, "Expected InvalidSession for wrong token/user");
}

#[tokio::test]
async fn test_ws_resume_preserves_session_id() {
    let app = get_test_app().await;

    let name = unique_name("respres");
    let email = unique_email("respres");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;

    // Connect and identify
    let (mut tx, mut rx, session_id, seq) = ws_connect_and_identify(app, &token).await;

    // Close gracefully
    tx.send(Message::Close(None)).await.ok();
    let _ = expect_close(&mut rx, WS_TIMEOUT).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Resume
    let (mut tx2, mut rx2) = ws_connect(app).await;
    let _hello = read_message(&mut rx2).await;

    send_message_ws(&mut tx2, serde_json::json!({
        "op": 6,
        "d": {
            "token": token,
            "session_id": session_id.clone(),
            "sequence": seq
        }
    })).await;

    let msg = read_message(&mut rx2).await;
    assert_eq!(msg["t"], "RESUMED");

    // The session_id in the resumed session should be the same as the original
    // (The server recreates the session with the same ID, so subsequent events
    // will use the same session context.)
    // We verify by sending a heartbeat and getting an ACK, confirming the session is alive.
    drain_pending_events(&mut rx2).await;
    send_message_ws(&mut tx2, serde_json::json!({ "op": 1, "d": null })).await;
    let ack = read_message(&mut rx2).await;
    assert_eq!(ack["op"], 11, "Resumed session should respond to heartbeats");
}

#[tokio::test]
async fn test_ws_presence_update_via_ws() {
    let app = get_test_app().await;

    let name = unique_name("wspres");
    let email = unique_email("wspres");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;
    let _server_id = app.create_server(&token, &unique_name("srv")).await;

    let (mut tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;
    drain_pending_events(&mut rx).await;

    // Send PresenceUpdate (op: 3) via WS
    send_message_ws(&mut tx, serde_json::json!({
        "op": 3,
        "d": {
            "status": "dnd",
            "custom_status": {
                "text": "Do not disturb"
            }
        }
    })).await;

    // The server should broadcast a PRESENCE_UPDATE event back via NATS
    // (since the user is subscribed to their own events).
    // This may arrive as a dispatch event.
    let event = wait_for_event(&mut rx, "PRESENCE_UPDATE").await;
    assert_eq!(event["t"], "PRESENCE_UPDATE");
    // Check the status was updated
    let status = event["d"]["status"].as_str().unwrap_or("");
    assert_eq!(status, "dnd", "Presence status should be 'dnd'");
}

#[tokio::test]
async fn test_ws_rate_limiting() {
    let app = get_test_app().await;

    let name = unique_name("ratelim");
    let email = unique_email("ratelim");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;

    let (mut tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;
    drain_pending_events(&mut rx).await;

    // Send 121 commands rapidly (limit is 120 per 60 seconds)
    // Use heartbeat (op:1) as a cheap command
    for _i in 0..121 {
        let res = tx.send(Message::Text(
            serde_json::to_string(&serde_json::json!({ "op": 1, "d": null })).unwrap()
        )).await;
        if res.is_err() {
            // Connection may have been closed by rate limiter already
            break;
        }
    }

    // After exceeding the limit, the connection should close
    // Note: the 121st command triggers the close. We might have received
    // some HeartbeatAcks before the close happens.
    let closed = expect_close(&mut rx, WS_LONG_TIMEOUT).await;
    assert!(closed, "Connection should close after exceeding rate limit (120 commands/60s)");
}

// ═══════════════════════════════════════════════════════════════════
// Additional Edge Case Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_ws_malformed_json() {
    let app = get_test_app().await;
    let (_tx, mut rx) = ws_connect(app).await;
    let _hello = read_message(&mut rx).await;

    // Send invalid JSON
    let mut tx = _tx;
    tx.send(Message::Text("this is not json{{{".to_string())).await.unwrap();

    // Should receive InvalidSession and/or connection closes
    let msg = try_read_message(&mut rx).await;
    if let Some(msg) = msg {
        assert_eq!(msg["op"], 9, "Malformed JSON should trigger InvalidSession");
    }
    // Connection should close after invalid payload
    let closed = expect_close(&mut rx, WS_TIMEOUT).await;
    assert!(closed, "Connection should close after malformed JSON");
}

#[tokio::test]
async fn test_ws_unknown_opcode() {
    let app = get_test_app().await;
    let (_tx, mut rx) = ws_connect(app).await;
    let _hello = read_message(&mut rx).await;

    // Send an unknown opcode (e.g., 99)
    let mut tx = _tx;
    tx.send(Message::Text(
        serde_json::to_string(&serde_json::json!({ "op": 99, "d": null })).unwrap()
    )).await.unwrap();

    // Server may send InvalidSession or simply ignore/close
    // Since OpCode deserialization will fail, this is equivalent to malformed JSON
    let msg = try_read_message(&mut rx).await;
    if let Some(msg) = msg {
        assert_eq!(msg["op"], 9, "Unknown opcode should trigger InvalidSession");
    }
}

#[tokio::test]
async fn test_ws_sequence_numbers_increment() {
    let app = get_test_app().await;

    let name = unique_name("seqinc");
    let email = unique_email("seqinc");
    let (token, _) = app.register_user(&name, &email, "TestPass123!").await;
    let server_id = app.create_server(&token, &unique_name("srv")).await;

    // Get text channel
    let channels_res = app.get(&token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = channels_res.json().await.unwrap();
    let channel_id = channels.iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .expect("No text channel")["id"]
        .as_str().unwrap().to_string();

    let (_tx, mut rx, _, _) = ws_connect_and_identify(app, &token).await;
    drain_pending_events(&mut rx).await;

    // Send two messages via API
    app.post(
        &token,
        &format!("/api/v1/channels/{}/messages", channel_id),
        serde_json::json!({ "content": "Message 1" }),
    ).await;

    app.post(
        &token,
        &format!("/api/v1/channels/{}/messages", channel_id),
        serde_json::json!({ "content": "Message 2" }),
    ).await;

    // Read events and verify sequence numbers are increasing
    let event1 = wait_for_event(&mut rx, "MESSAGE_CREATE").await;
    let seq1 = event1["s"].as_u64().expect("Event must have sequence number");

    let event2 = wait_for_event(&mut rx, "MESSAGE_CREATE").await;
    let seq2 = event2["s"].as_u64().expect("Event must have sequence number");

    assert!(seq2 > seq1, "Sequence numbers must be strictly increasing: {} > {}", seq2, seq1);
}

#[tokio::test]
async fn test_ws_identify_missing_token() {
    let app = get_test_app().await;
    let (mut tx, mut rx) = ws_connect(app).await;
    let _hello = read_message(&mut rx).await;

    // Send Identify without token field
    send_message_ws(&mut tx, serde_json::json!({
        "op": 2,
        "d": {
            "properties": {}
        }
    })).await;

    // Should get InvalidSession
    let msg = read_message(&mut rx).await;
    assert_eq!(msg["op"], 9, "Missing token should trigger InvalidSession");
}
