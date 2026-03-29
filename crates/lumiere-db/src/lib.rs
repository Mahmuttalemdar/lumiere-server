use lumiere_models::config::AppConfig;
use scylla::prepared_statement::PreparedStatement;
use scylla::Session;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

/// Column list for SELECT queries on the messages table.
pub const MSG_COLS: &str = "message_id, channel_id, author_id, content, type, flags, edited_at, \
     pinned, mention_everyone, mentions, mention_roles, embeds, attachments, reference_id, deleted";

/// All CQL prepared statements, created once at startup.
pub struct ScyllaPrepared {
    // ── Messages: reads ──────────────────────────────────────────
    pub get_messages_before: PreparedStatement,
    pub get_messages_after: PreparedStatement,
    pub get_messages_latest: PreparedStatement,
    pub get_message_by_id: PreparedStatement,
    pub get_message_author: PreparedStatement,

    // ── Messages: writes ─────────────────────────────────────────
    pub insert_message: PreparedStatement,
    pub insert_webhook_message: PreparedStatement,
    pub update_content: PreparedStatement,
    pub update_embeds: PreparedStatement,
    pub soft_delete: PreparedStatement,

    // ── Pins ─────────────────────────────────────────────────────
    pub get_pin_ids: PreparedStatement,
    pub count_pins: PreparedStatement,
    pub check_message_exists: PreparedStatement,
    pub set_pinned: PreparedStatement,
    pub insert_pin: PreparedStatement,
    pub unset_pinned: PreparedStatement,
    pub delete_pin: PreparedStatement,

    // ── Reactions ────────────────────────────────────────────────
    pub insert_reaction: PreparedStatement,
    pub delete_reaction: PreparedStatement,
    pub get_reactors: PreparedStatement,
    pub get_reactors_after: PreparedStatement,
    pub delete_all_reactions: PreparedStatement,
    pub delete_emoji_reactions: PreparedStatement,

    // ── Read states / typing ─────────────────────────────────────
    pub upsert_read_state: PreparedStatement,
    pub get_unread: PreparedStatement,
}

/// Prepare all CQL statements against the current ScyllaDB session.
async fn prepare_statements(session: &Session) -> anyhow::Result<ScyllaPrepared> {
    let p = |cql: &str| {
        let session = session;
        let cql = cql.to_owned();
        async move {
            session
                .prepare(cql)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to prepare CQL: {}", e))
        }
    };

    Ok(ScyllaPrepared {
        // ── Messages: reads
        get_messages_before: p(&format!(
            "SELECT {} FROM messages WHERE channel_id = ? AND bucket = ? AND message_id < ? ORDER BY message_id DESC LIMIT ?",
            MSG_COLS
        )).await?,
        get_messages_after: p(&format!(
            "SELECT {} FROM messages WHERE channel_id = ? AND bucket = ? AND message_id > ? ORDER BY message_id ASC LIMIT ?",
            MSG_COLS
        )).await?,
        get_messages_latest: p(&format!(
            "SELECT {} FROM messages WHERE channel_id = ? AND bucket = ? ORDER BY message_id DESC LIMIT ?",
            MSG_COLS
        )).await?,
        get_message_by_id: p(&format!(
            "SELECT {} FROM messages WHERE channel_id = ? AND bucket = ? AND message_id = ?",
            MSG_COLS
        )).await?,
        get_message_author: p(
            "SELECT author_id FROM messages WHERE channel_id = ? AND bucket = ? AND message_id = ?"
        ).await?,

        // ── Messages: writes
        insert_message: p(
            "INSERT INTO messages (channel_id, bucket, message_id, author_id, content, type, flags, \
             pinned, mention_everyone, mentions, mention_roles, embeds, attachments, reference_id, deleted) \
             VALUES (?, ?, ?, ?, ?, ?, 0, false, ?, ?, ?, ?, ?, ?, false)"
        ).await?,
        insert_webhook_message: p(
            "INSERT INTO messages (channel_id, bucket, message_id, author_id, content, type, flags, \
             pinned, mention_everyone, deleted) VALUES (?, ?, ?, ?, ?, 0, 0, false, false, false)"
        ).await?,
        update_content: p(
            "UPDATE messages SET content = ?, edited_at = ? WHERE channel_id = ? AND bucket = ? AND message_id = ?"
        ).await?,
        update_embeds: p(
            "UPDATE messages SET embeds = ?, edited_at = ? WHERE channel_id = ? AND bucket = ? AND message_id = ?"
        ).await?,
        soft_delete: p(
            "UPDATE messages SET deleted = true WHERE channel_id = ? AND bucket = ? AND message_id = ?"
        ).await?,

        // ── Pins
        get_pin_ids: p(
            "SELECT message_id FROM pins WHERE channel_id = ?"
        ).await?,
        count_pins: p(
            "SELECT COUNT(*) FROM pins WHERE channel_id = ?"
        ).await?,
        check_message_exists: p(
            "SELECT message_id, deleted FROM messages WHERE channel_id = ? AND bucket = ? AND message_id = ?"
        ).await?,
        set_pinned: p(
            "UPDATE messages SET pinned = true WHERE channel_id = ? AND bucket = ? AND message_id = ?"
        ).await?,
        insert_pin: p(
            "INSERT INTO pins (channel_id, message_id, pinned_by, pinned_at) VALUES (?, ?, ?, ?)"
        ).await?,
        unset_pinned: p(
            "UPDATE messages SET pinned = false WHERE channel_id = ? AND bucket = ? AND message_id = ?"
        ).await?,
        delete_pin: p(
            "DELETE FROM pins WHERE channel_id = ? AND message_id = ?"
        ).await?,

        // ── Reactions
        insert_reaction: p(
            "INSERT INTO reactions (channel_id, message_id, emoji, user_id, created_at) VALUES (?, ?, ?, ?, ?)"
        ).await?,
        delete_reaction: p(
            "DELETE FROM reactions WHERE channel_id = ? AND message_id = ? AND emoji = ? AND user_id = ?"
        ).await?,
        get_reactors: p(
            "SELECT user_id FROM reactions WHERE channel_id = ? AND message_id = ? AND emoji = ? LIMIT ?"
        ).await?,
        get_reactors_after: p(
            "SELECT user_id FROM reactions WHERE channel_id = ? AND message_id = ? AND emoji = ? AND user_id > ? LIMIT ?"
        ).await?,
        delete_all_reactions: p(
            "DELETE FROM reactions WHERE channel_id = ? AND message_id = ?"
        ).await?,
        delete_emoji_reactions: p(
            "DELETE FROM reactions WHERE channel_id = ? AND message_id = ? AND emoji = ?"
        ).await?,

        // ── Read states
        upsert_read_state: p(
            "INSERT INTO read_states (user_id, channel_id, last_message_id, mention_count, updated_at) \
             VALUES (?, ?, ?, 0, ?)"
        ).await?,
        get_unread: p(
            "SELECT channel_id, last_message_id, mention_count FROM read_states WHERE user_id = ? LIMIT 1000"
        ).await?,
    })
}

#[derive(Clone)]
pub struct Database {
    pub pg: PgPool,
    pub scylla: Arc<Session>,
    prepared: Option<Arc<ScyllaPrepared>>,
}

impl Database {
    pub async fn connect(config: &AppConfig) -> anyhow::Result<Self> {
        let pg = PgPoolOptions::new()
            .max_connections(config.database.max_connections)
            .min_connections(config.database.min_connections)
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(1800))
            .connect(&config.database.url)
            .await?;

        tracing::info!("Connected to PostgreSQL");

        let scylla = scylla::SessionBuilder::new()
            .known_nodes(&config.scylla.nodes)
            .build()
            .await?;

        tracing::info!("Connected to ScyllaDB");

        Ok(Self {
            pg,
            scylla: Arc::new(scylla),
            prepared: None,
        })
    }

    /// Prepare all CQL statements. Must be called after migrations
    /// (so the keyspace and tables exist).
    pub async fn prepare_all(&mut self) -> anyhow::Result<()> {
        tracing::info!("Preparing ScyllaDB statements...");
        let stmts = prepare_statements(&self.scylla).await?;
        self.prepared = Some(Arc::new(stmts));
        tracing::info!("ScyllaDB prepared statements ready");
        Ok(())
    }

    /// Access the prepared statements. Panics if `prepare_all()` was not called.
    pub fn prepared(&self) -> &ScyllaPrepared {
        self.prepared
            .as_ref()
            .expect("ScyllaPrepared not initialized — call prepare_all() after migrations")
    }

    pub async fn run_pg_migrations(&self) -> anyhow::Result<()> {
        tracing::info!("Running PostgreSQL migrations...");
        sqlx::migrate!("../../migrations/postgres")
            .run(&self.pg)
            .await?;
        tracing::info!("PostgreSQL migrations complete");
        Ok(())
    }

    pub async fn run_scylla_migrations(&self, config: &AppConfig) -> anyhow::Result<()> {
        tracing::info!("Running ScyllaDB migrations...");

        // Validate keyspace name to prevent CQL injection
        if !config.scylla.keyspace.chars().all(|c| c.is_alphanumeric() || c == '_') {
            anyhow::bail!("Invalid keyspace name: {}", config.scylla.keyspace);
        }

        // Create keyspace if not exists
        let create_keyspace = format!(
            "CREATE KEYSPACE IF NOT EXISTS {} WITH replication = {{'class': 'SimpleStrategy', 'replication_factor': {}}}",
            config.scylla.keyspace, config.scylla.replication_factor
        );
        self.scylla
            .query_unpaged(create_keyspace, &[])
            .await?;

        self.scylla
            .use_keyspace(&config.scylla.keyspace, false)
            .await?;

        // Read and execute .cql migration files in order
        let mut entries: Vec<_> = std::fs::read_dir("migrations/scylla")?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "cql")
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let content = std::fs::read_to_string(entry.path())?;
            for statement in content.split(';') {
                // Strip comment lines from the statement
                let stmt: String = statement
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("--"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let stmt = stmt.trim();
                if stmt.is_empty() {
                    continue;
                }
                // Skip USE statements (we already set keyspace)
                if stmt.to_uppercase().starts_with("USE ") {
                    continue;
                }
                // Skip CREATE KEYSPACE (already handled above)
                if stmt.to_uppercase().starts_with("CREATE KEYSPACE") {
                    continue;
                }
                tracing::debug!(statement = stmt, "Executing CQL migration");
                self.scylla
                    .query_unpaged(stmt.to_string(), &[])
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("CQL migration failed: {} — statement: {}", e, stmt)
                    })?;
            }
            tracing::info!(file = %entry.file_name().to_string_lossy(), "CQL migration applied");
        }

        tracing::info!("ScyllaDB migrations complete");
        Ok(())
    }

    pub async fn check_pg_health(&self) -> bool {
        sqlx::query("SELECT 1")
            .execute(&self.pg)
            .await
            .is_ok()
    }

    pub async fn check_scylla_health(&self) -> bool {
        self.scylla
            .query_unpaged("SELECT now() FROM system.local", &[])
            .await
            .is_ok()
    }
}
