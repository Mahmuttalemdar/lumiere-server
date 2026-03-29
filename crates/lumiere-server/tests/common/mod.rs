use lumiere_models::config::AppConfig;
use lumiere_server::{build_app_state, build_router, AppState};
use std::sync::Arc;
use tokio::sync::OnceCell as AsyncOnceCell;

static APP: AsyncOnceCell<TestApp> = AsyncOnceCell::const_new();

/// Shared test application — initialized once per test run
pub struct TestApp {
    pub state: Arc<AppState>,
    pub addr: String,
    pub client: reqwest::Client,
}

impl TestApp {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// Register a new user and return (access_token, user_id)
    pub async fn register_user(&self, username: &str, email: &str, password: &str) -> (String, i64) {
        let res = self.client
            .post(&self.url("/api/v1/auth/register"))
            .json(&serde_json::json!({
                "username": username,
                "email": email,
                "password": password
            }))
            .send()
            .await
            .expect("register request failed");

        let status = res.status();
        let body: serde_json::Value = res.json().await.expect("parse register response");
        assert_eq!(status, 201, "register failed: {:?}", body);

        let token = body["access_token"].as_str().unwrap().to_string();
        let user_id = body["user"]["id"].as_str().unwrap().parse::<i64>().unwrap();
        (token, user_id)
    }

    /// Login and return (access_token, refresh_token)
    pub async fn login(&self, email: &str, password: &str) -> (String, String) {
        let res = self.client
            .post(&self.url("/api/v1/auth/login"))
            .json(&serde_json::json!({
                "email": email,
                "password": password
            }))
            .send()
            .await
            .expect("login request failed");

        let body: serde_json::Value = res.json().await.expect("parse login response");
        let access = body["access_token"].as_str().unwrap().to_string();
        let refresh = body["refresh_token"].as_str().unwrap().to_string();
        (access, refresh)
    }

    /// Create a server and return (server_id)
    pub async fn create_server(&self, token: &str, name: &str) -> i64 {
        let res = self.client
            .post(&self.url("/api/v1/servers"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .expect("create server request failed");

        let body: serde_json::Value = res.json().await.expect("parse server response");
        body["id"].as_str().unwrap().parse::<i64>().unwrap()
    }

    /// Authenticated GET
    pub async fn get(&self, token: &str, path: &str) -> reqwest::Response {
        self.client
            .get(&self.url(path))
            .bearer_auth(token)
            .send()
            .await
            .expect("GET request failed")
    }

    /// Authenticated POST with JSON body
    pub async fn post(&self, token: &str, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.client
            .post(&self.url(path))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .expect("POST request failed")
    }

    /// Authenticated PATCH with JSON body
    pub async fn patch(&self, token: &str, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.client
            .patch(&self.url(path))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .expect("PATCH request failed")
    }

    /// Authenticated PUT with JSON body
    pub async fn put(&self, token: &str, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.client
            .put(&self.url(path))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .expect("PUT request failed")
    }

    /// Authenticated DELETE
    pub async fn delete(&self, token: &str, path: &str) -> reqwest::Response {
        self.client
            .delete(&self.url(path))
            .bearer_auth(token)
            .send()
            .await
            .expect("DELETE request failed")
    }

    /// Authenticated DELETE with JSON body
    pub async fn delete_with_body(&self, token: &str, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.client
            .delete(&self.url(path))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .expect("DELETE request failed")
    }

    /// Unauthenticated GET
    pub async fn get_unauth(&self, path: &str) -> reqwest::Response {
        self.client
            .get(&self.url(path))
            .send()
            .await
            .expect("GET request failed")
    }

    /// Unauthenticated POST
    pub async fn post_unauth(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.client
            .post(&self.url(path))
            .json(&body)
            .send()
            .await
            .expect("POST request failed")
    }
}

/// Get or initialize the shared test app.
/// Uses test config (config/test.toml) and connects to test infrastructure.
/// Requires `docker compose -f docker-compose.test.yml up -d` to be running.
pub async fn get_test_app() -> &'static TestApp {
    APP.get_or_init(|| async {
        // Set env to test
        std::env::set_var("LUMIERE_ENV", "test");

        // Ensure working directory is workspace root (where config/ lives)
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let workspace_root = std::path::Path::new(manifest_dir).parent().unwrap().parent().unwrap();
        std::env::set_current_dir(workspace_root).expect("Failed to set working directory to workspace root");

        // Load test config
        let config = AppConfig::load().expect("Failed to load test config");

        // Build app state
        let state = build_app_state(config).await.expect("Failed to build test app state");

        // Clean database for test isolation
        clean_database(&state).await;

        // Build router
        let app = build_router(state.clone());

        // Bind to random port using std::net (not tokio) so we can read the address
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("Failed to bind test server");
        let addr = std_listener.local_addr().unwrap().to_string();
        std_listener.set_nonblocking(true).unwrap();

        // Start server in a dedicated thread with its own runtime
        // (each #[tokio::test] creates a new runtime, but we need the server to outlive all tests)
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
                axum::serve(listener, app).await.ok();
            });
        });

        // Wait for server to be ready
        let client = reqwest::Client::new();
        for _ in 0..50 {
            if client.get(&format!("http://{}/health", addr)).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        TestApp { state, addr, client }
    })
    .await
}

/// Clean all tables for test isolation
async fn clean_database(state: &AppState) {
    // PostgreSQL — truncate all tables (order matters due to FK constraints)
    let tables = [
        "member_roles", "server_members", "dm_recipients", "permission_overrides",
        "invites", "bans", "warnings", "webhooks", "emojis", "application_commands",
        "applications", "auto_mod_rules", "audit_log", "notification_settings",
        "device_tokens", "reports", "attachments", "user_notes", "relationships",
        "user_settings", "roles", "channels", "servers", "users",
    ];
    for table in tables {
        let sql = format!("TRUNCATE TABLE {} CASCADE", table);
        let _ = sqlx::query(&sql).execute(&state.db.pg).await;
    }

    // ScyllaDB — truncate tables
    let scylla_tables = ["messages", "read_states", "pins", "reactions", "encrypted_messages"];
    for table in scylla_tables {
        let _ = state.db.scylla.query_unpaged(format!("TRUNCATE {}", table), &[]).await;
    }

    // Redis — flush test database
    let mut conn = state.redis.clone();
    let _: Result<(), _> = redis::cmd("FLUSHDB").query_async(&mut conn).await;
}

/// Generate a unique username for tests
pub fn unique_name(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}", prefix, n)
}

/// Generate a unique email for tests
pub fn unique_email(prefix: &str) -> String {
    format!("{}@test.lumiere.dev", unique_name(prefix))
}
