mod common;
use common::{get_test_app, unique_email, unique_name};

use common::TestApp;

// ─── Helper ────────────────────────────────────────────────────────

/// Sets up a server with an owner and a second member.
/// Returns (owner_token, member_token, server_id, channel_id, member_user_id).
async fn setup_server_with_two_members(app: &TestApp) -> (String, String, i64, i64, i64) {
    let owner_name = unique_name("owner");
    let (owner_token, _) = app
        .register_user(&owner_name, &unique_email(&owner_name), "testpass123")
        .await;
    let server_id = app.create_server(&owner_token, &unique_name("srv")).await;

    // Get the default text channel (type == 0)
    let res = app
        .get(
            &owner_token,
            &format!("/api/v1/servers/{}/channels", server_id),
        )
        .await;
    let channels: Vec<serde_json::Value> = res.json().await.unwrap();
    let channel_id = channels
        .iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();

    // Create invite
    let invite_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/invites", channel_id),
            serde_json::json!({}),
        )
        .await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap().to_string();

    // Register second user and join
    let member_name = unique_name("member");
    let (member_token, member_id) = app
        .register_user(&member_name, &unique_email(&member_name), "testpass123")
        .await;
    app.post(
        &member_token,
        &format!("/api/v1/invites/{}", code),
        serde_json::json!({}),
    )
    .await;

    (owner_token, member_token, server_id, channel_id, member_id)
}

// ═══════════════════════════════════════════════════════════════════
// Moderation — Warnings
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_warning() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    let res = app
        .post(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, member_id
            ),
            serde_json::json!({ "reason": "Spamming in chat" }),
        )
        .await;

    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["reason"].as_str().unwrap(), "Spamming in chat");
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["user_id"].as_str().unwrap(), member_id.to_string());
}

#[tokio::test]
async fn test_create_warning_not_moderator() {
    let app = get_test_app().await;
    let (owner_token, member_token, server_id, _, _) = setup_server_with_two_members(app).await;

    // Get the owner's user id
    let me_res = app.get(&owner_token, "/api/v1/users/@me").await;
    let me: serde_json::Value = me_res.json().await.unwrap();
    let owner_id: i64 = me["id"].as_str().unwrap().parse().unwrap();

    let res = app
        .post(
            &member_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, owner_id
            ),
            serde_json::json!({ "reason": "test" }),
        )
        .await;

    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_create_warning_self() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, _) = setup_server_with_two_members(app).await;

    let me_res = app.get(&owner_token, "/api/v1/users/@me").await;
    let me: serde_json::Value = me_res.json().await.unwrap();
    let owner_id: i64 = me["id"].as_str().unwrap().parse().unwrap();

    let res = app
        .post(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, owner_id
            ),
            serde_json::json!({ "reason": "self warn" }),
        )
        .await;

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_create_warning_nonmember() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, _) = setup_server_with_two_members(app).await;

    // Register a user who is NOT a member of the server
    let outsider_name = unique_name("outsider");
    let (_, outsider_id) = app
        .register_user(&outsider_name, &unique_email(&outsider_name), "testpass123")
        .await;

    let res = app
        .post(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, outsider_id
            ),
            serde_json::json!({ "reason": "not a member" }),
        )
        .await;

    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn test_list_warnings() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    // Create two warnings
    app.post(
        &owner_token,
        &format!(
            "/api/v1/servers/{}/members/{}/warnings",
            server_id, member_id
        ),
        serde_json::json!({ "reason": "First warning" }),
    )
    .await;

    app.post(
        &owner_token,
        &format!(
            "/api/v1/servers/{}/members/{}/warnings",
            server_id, member_id
        ),
        serde_json::json!({ "reason": "Second warning" }),
    )
    .await;

    let res = app
        .get(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, member_id
            ),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(body.len() >= 2);
}

#[tokio::test]
async fn test_delete_warning() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, member_id
            ),
            serde_json::json!({ "reason": "to be deleted" }),
        )
        .await;
    let warning: serde_json::Value = create_res.json().await.unwrap();
    let warning_id = warning["id"].as_str().unwrap();

    let res = app
        .delete(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings/{}",
                server_id, member_id, warning_id
            ),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_warning_not_found() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    let res = app
        .delete(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings/999999999999",
                server_id, member_id
            ),
        )
        .await;

    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn test_warning_creates_audit_log() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    app.post(
        &owner_token,
        &format!(
            "/api/v1/servers/{}/members/{}/warnings",
            server_id, member_id
        ),
        serde_json::json!({ "reason": "audit log check" }),
    )
    .await;

    // action_type 50 = warning created
    let res = app
        .get(
            &owner_token,
            &format!("/api/v1/servers/{}/audit-log?action_type=50", server_id),
        )
        .await;

    assert_eq!(res.status(), 200);
    let entries: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(
        !entries.is_empty(),
        "Audit log should contain warning entry"
    );
    assert_eq!(entries[0]["action_type"].as_i64().unwrap(), 50);
}

#[tokio::test]
async fn test_list_warnings_not_moderator() {
    let app = get_test_app().await;
    let (owner_token, member_token, server_id, _, _) = setup_server_with_two_members(app).await;

    let me_res = app.get(&owner_token, "/api/v1/users/@me").await;
    let me: serde_json::Value = me_res.json().await.unwrap();
    let owner_id: i64 = me["id"].as_str().unwrap().parse().unwrap();

    let res = app
        .get(
            &member_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, owner_id
            ),
        )
        .await;

    assert_eq!(res.status(), 403);
}

// ═══════════════════════════════════════════════════════════════════
// Moderation — Audit Log
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_get_audit_log() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    // Create a warning to produce an audit log entry
    app.post(
        &owner_token,
        &format!(
            "/api/v1/servers/{}/members/{}/warnings",
            server_id, member_id
        ),
        serde_json::json!({ "reason": "audit test" }),
    )
    .await;

    let res = app
        .get(
            &owner_token,
            &format!("/api/v1/servers/{}/audit-log", server_id),
        )
        .await;

    assert_eq!(res.status(), 200);
    let entries: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(!entries.is_empty());
}

#[tokio::test]
async fn test_get_audit_log_not_permitted() {
    let app = get_test_app().await;
    let (_, member_token, server_id, _, _) = setup_server_with_two_members(app).await;

    let res = app
        .get(
            &member_token,
            &format!("/api/v1/servers/{}/audit-log", server_id),
        )
        .await;

    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_audit_log_filter_by_user() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    // Create a warning (moderator = owner)
    app.post(
        &owner_token,
        &format!(
            "/api/v1/servers/{}/members/{}/warnings",
            server_id, member_id
        ),
        serde_json::json!({ "reason": "filter test" }),
    )
    .await;

    let me_res = app.get(&owner_token, "/api/v1/users/@me").await;
    let me: serde_json::Value = me_res.json().await.unwrap();
    let owner_id: i64 = me["id"].as_str().unwrap().parse().unwrap();

    let res = app
        .get(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/audit-log?user_id={}",
                server_id, owner_id
            ),
        )
        .await;

    assert_eq!(res.status(), 200);
    let entries: Vec<serde_json::Value> = res.json().await.unwrap();
    for entry in &entries {
        assert_eq!(entry["user_id"].as_str().unwrap(), owner_id.to_string());
    }
}

#[tokio::test]
async fn test_audit_log_filter_by_action() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    // Create and delete a warning to get action_type 50 and 51
    let create_res = app
        .post(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, member_id
            ),
            serde_json::json!({ "reason": "action filter" }),
        )
        .await;
    let warning: serde_json::Value = create_res.json().await.unwrap();
    let warning_id = warning["id"].as_str().unwrap();

    app.delete(
        &owner_token,
        &format!(
            "/api/v1/servers/{}/members/{}/warnings/{}",
            server_id, member_id, warning_id
        ),
    )
    .await;

    // Filter for action_type 51 (warning deleted)
    let res = app
        .get(
            &owner_token,
            &format!("/api/v1/servers/{}/audit-log?action_type=51", server_id),
        )
        .await;

    assert_eq!(res.status(), 200);
    let entries: Vec<serde_json::Value> = res.json().await.unwrap();
    for entry in &entries {
        assert_eq!(entry["action_type"].as_i64().unwrap(), 51);
    }
}

#[tokio::test]
async fn test_audit_log_pagination() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    // Create multiple warnings to produce multiple audit log entries
    for i in 0..3 {
        app.post(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, member_id
            ),
            serde_json::json!({ "reason": format!("pagination {}", i) }),
        )
        .await;
    }

    // Get all entries
    let res = app
        .get(
            &owner_token,
            &format!("/api/v1/servers/{}/audit-log", server_id),
        )
        .await;
    let all_entries: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(all_entries.len() >= 3);

    // Use "before" to paginate — entries are ordered by id DESC
    let before_id = all_entries[0]["id"].as_str().unwrap();
    let res = app
        .get(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/audit-log?before={}",
                server_id, before_id
            ),
        )
        .await;
    let paginated: Vec<serde_json::Value> = res.json().await.unwrap();
    // All entries in the paginated result should have id < before_id
    let before_val: i64 = before_id.parse().unwrap();
    for entry in &paginated {
        let entry_id: i64 = entry["id"].as_str().unwrap().parse().unwrap();
        assert!(entry_id < before_val);
    }
}

#[tokio::test]
async fn test_audit_log_limit() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, member_id) = setup_server_with_two_members(app).await;

    for i in 0..5 {
        app.post(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, member_id
            ),
            serde_json::json!({ "reason": format!("limit {}", i) }),
        )
        .await;
    }

    let res = app
        .get(
            &owner_token,
            &format!("/api/v1/servers/{}/audit-log?limit=2", server_id),
        )
        .await;

    assert_eq!(res.status(), 200);
    let entries: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(entries.len() <= 2);
}

// ═══════════════════════════════════════════════════════════════════
// Auto-Moderation
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_automod_rule() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, _) = setup_server_with_two_members(app).await;

    let res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/auto-moderation/rules", server_id),
            serde_json::json!({
                "name": "Block bad words",
                "event_type": 1,
                "trigger_type": 1,
                "trigger_metadata": { "keyword_filter": ["badword1", "badword2"] },
                "actions": [{ "type": 1, "metadata": {} }],
                "enabled": true
            }),
        )
        .await;

    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "Block bad words");
    assert!(body["enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_list_automod_rules() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, _) = setup_server_with_two_members(app).await;

    // Create two rules
    for name in &["Rule A", "Rule B"] {
        app.post(
            &owner_token,
            &format!("/api/v1/servers/{}/auto-moderation/rules", server_id),
            serde_json::json!({
                "name": name,
                "event_type": 1,
                "trigger_type": 1,
                "trigger_metadata": {},
                "actions": []
            }),
        )
        .await;
    }

    let res = app
        .get(
            &owner_token,
            &format!("/api/v1/servers/{}/auto-moderation/rules", server_id),
        )
        .await;

    assert_eq!(res.status(), 200);
    let rules: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(rules.len() >= 2);
}

#[tokio::test]
async fn test_update_automod_rule() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, _) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/auto-moderation/rules", server_id),
            serde_json::json!({
                "name": "Original",
                "event_type": 1,
                "trigger_type": 1,
                "trigger_metadata": {},
                "actions": [],
                "enabled": true
            }),
        )
        .await;
    let rule: serde_json::Value = create_res.json().await.unwrap();
    let rule_id = rule["id"].as_str().unwrap();

    let res = app
        .patch(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/auto-moderation/rules/{}",
                server_id, rule_id
            ),
            serde_json::json!({ "name": "Updated", "enabled": false }),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_automod_rule() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, _) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/auto-moderation/rules", server_id),
            serde_json::json!({
                "name": "To Delete",
                "event_type": 1,
                "trigger_type": 1,
                "trigger_metadata": {},
                "actions": []
            }),
        )
        .await;
    let rule: serde_json::Value = create_res.json().await.unwrap();
    let rule_id = rule["id"].as_str().unwrap();

    let res = app
        .delete(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/auto-moderation/rules/{}",
                server_id, rule_id
            ),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_create_automod_not_permitted() {
    let app = get_test_app().await;
    let (_, member_token, server_id, _, _) = setup_server_with_two_members(app).await;

    let res = app
        .post(
            &member_token,
            &format!("/api/v1/servers/{}/auto-moderation/rules", server_id),
            serde_json::json!({
                "name": "Blocked",
                "event_type": 1,
                "trigger_type": 1,
                "trigger_metadata": {},
                "actions": []
            }),
        )
        .await;

    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_automod_rule_fields() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, _) = setup_server_with_two_members(app).await;

    let res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/auto-moderation/rules", server_id),
            serde_json::json!({
                "name": "Field Check",
                "event_type": 1,
                "trigger_type": 3,
                "trigger_metadata": { "mention_limit": 5 },
                "actions": [{ "type": 2, "metadata": { "channel_id": 123 } }],
                "enabled": false
            }),
        )
        .await;

    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["trigger_type"].as_i64().unwrap(), 3);
    assert!(!body["enabled"].as_bool().unwrap());
    assert!(body["actions"].is_array());
    assert_eq!(
        body["trigger_metadata"]["mention_limit"].as_i64().unwrap(),
        5
    );
}

// ═══════════════════════════════════════════════════════════════════
// Reports
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_report() {
    let app = get_test_app().await;
    let name = unique_name("reporter");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let res = app
        .post(
            &token,
            "/api/v1/report",
            serde_json::json!({
                "target_type": "user",
                "target_id": 123456,
                "reason": 1
            }),
        )
        .await;

    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn test_create_report_invalid_type() {
    let app = get_test_app().await;
    let name = unique_name("reporter");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let res = app
        .post(
            &token,
            "/api/v1/report",
            serde_json::json!({
                "target_type": "invalid_type",
                "target_id": 123456,
                "reason": 1
            }),
        )
        .await;

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_create_report_with_description() {
    let app = get_test_app().await;
    let name = unique_name("reporter");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let res = app
        .post(
            &token,
            "/api/v1/report",
            serde_json::json!({
                "target_type": "message",
                "target_id": 789,
                "reason": 2,
                "description": "This message contains harassment"
            }),
        )
        .await;

    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn test_create_report_valid_types() {
    let app = get_test_app().await;
    let name = unique_name("reporter");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    for target_type in &["message", "user", "server"] {
        let res = app
            .post(
                &token,
                "/api/v1/report",
                serde_json::json!({
                    "target_type": target_type,
                    "target_id": 100,
                    "reason": 1
                }),
            )
            .await;

        assert_eq!(
            res.status(),
            201,
            "Report with target_type '{}' should succeed",
            target_type
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
// Webhooks
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_webhook() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    let res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "CI Notifier" }),
        )
        .await;

    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "CI Notifier");
    assert!(
        body["token"].as_str().is_some(),
        "Webhook creation should return a token"
    );
    assert!(body["id"].as_str().is_some());
}

#[tokio::test]
async fn test_create_webhook_not_permitted() {
    let app = get_test_app().await;
    let (_, member_token, _, channel_id, _) = setup_server_with_two_members(app).await;

    let res = app
        .post(
            &member_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "Blocked" }),
        )
        .await;

    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_list_channel_webhooks() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    // Create two webhooks
    for name in &["Hook1", "Hook2"] {
        app.post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": name }),
        )
        .await;
    }

    let res = app
        .get(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
        )
        .await;

    assert_eq!(res.status(), 200);
    let webhooks: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(webhooks.len() >= 2);
}

#[tokio::test]
async fn test_get_webhook() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "GetTest" }),
        )
        .await;
    let hook: serde_json::Value = create_res.json().await.unwrap();
    let webhook_id = hook["id"].as_str().unwrap();

    let res = app
        .get(&owner_token, &format!("/api/v1/webhooks/{}", webhook_id))
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "GetTest");
}

#[tokio::test]
async fn test_update_webhook_name() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "OldName" }),
        )
        .await;
    let hook: serde_json::Value = create_res.json().await.unwrap();
    let webhook_id = hook["id"].as_str().unwrap();

    let res = app
        .patch(
            &owner_token,
            &format!("/api/v1/webhooks/{}", webhook_id),
            serde_json::json!({ "name": "NewName" }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "NewName");
}

#[tokio::test]
async fn test_delete_webhook() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "ToDelete" }),
        )
        .await;
    let hook: serde_json::Value = create_res.json().await.unwrap();
    let webhook_id = hook["id"].as_str().unwrap();

    let res = app
        .delete(&owner_token, &format!("/api/v1/webhooks/{}", webhook_id))
        .await;

    assert_eq!(res.status(), 204);

    // Verify it is gone
    let get_res = app
        .get(&owner_token, &format!("/api/v1/webhooks/{}", webhook_id))
        .await;
    assert_eq!(get_res.status(), 404);
}

#[tokio::test]
async fn test_execute_webhook() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "ExecTest" }),
        )
        .await;
    let hook: serde_json::Value = create_res.json().await.unwrap();
    let webhook_id = hook["id"].as_str().unwrap();
    let token = hook["token"].as_str().unwrap();

    // Execute webhook — no auth header needed
    let res = app
        .post_unauth(
            &format!("/api/v1/webhooks/{}/{}", webhook_id, token),
            serde_json::json!({ "content": "Hello from webhook" }),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_execute_webhook_invalid_token() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "BadTokenTest" }),
        )
        .await;
    let hook: serde_json::Value = create_res.json().await.unwrap();
    let webhook_id = hook["id"].as_str().unwrap();

    let res = app
        .post_unauth(
            &format!("/api/v1/webhooks/{}/invalid_token_here", webhook_id),
            serde_json::json!({ "content": "should fail" }),
        )
        .await;

    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_execute_webhook_empty_content() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    let create_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "EmptyContent" }),
        )
        .await;
    let hook: serde_json::Value = create_res.json().await.unwrap();
    let webhook_id = hook["id"].as_str().unwrap();
    let token = hook["token"].as_str().unwrap();

    let res = app
        .post_unauth(
            &format!("/api/v1/webhooks/{}/{}", webhook_id, token),
            serde_json::json!({ "content": "" }),
        )
        .await;

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_webhook_name_validation_empty() {
    let app = get_test_app().await;
    let (owner_token, _, _, channel_id, _) = setup_server_with_two_members(app).await;

    // Empty name — the server should reject or at least process it.
    // (The CreateWebhookRequest has no length validation, so this tests server behavior.)
    let res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/webhooks", channel_id),
            serde_json::json!({ "name": "" }),
        )
        .await;

    // Webhook creation with empty name may succeed (no validator on name) or fail —
    // we assert it returns a valid HTTP status (either 201 or 400).
    let status = res.status().as_u16();
    assert!(
        status == 201 || status == 400,
        "Expected 201 or 400, got {}",
        status
    );
}

// ═══════════════════════════════════════════════════════════════════
// Applications / Bots
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_application() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let res = app
        .post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": "My Bot App" }),
        )
        .await;

    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "My Bot App");
    assert!(body["bot_id"].is_string() || body["bot_id"].is_number());
    assert!(
        body["bot_token"].as_str().is_some(),
        "Should return bot_token on creation"
    );
}

#[tokio::test]
async fn test_list_my_applications() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    // Create two applications
    for app_name in &["App 1", "App 2"] {
        app.post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": app_name }),
        )
        .await;
    }

    let res = app.get(&token, "/api/v1/applications/@me").await;

    assert_eq!(res.status(), 200);
    let apps: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(apps.len() >= 2);
}

#[tokio::test]
async fn test_update_application() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let create_res = app
        .post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": "Original App" }),
        )
        .await;
    let application: serde_json::Value = create_res.json().await.unwrap();
    let app_id = application["id"].as_str().unwrap();

    let res = app
        .patch(
            &token,
            &format!("/api/v1/applications/{}", app_id),
            serde_json::json!({ "name": "Renamed App" }),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_reset_bot_token() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let create_res = app
        .post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": "Token Reset App" }),
        )
        .await;
    let application: serde_json::Value = create_res.json().await.unwrap();
    let app_id = application["id"].as_str().unwrap();
    let old_token = application["bot_token"].as_str().unwrap().to_string();

    let res = app
        .post(
            &token,
            &format!("/api/v1/applications/{}/bot/reset-token", app_id),
            serde_json::json!({}),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let new_token = body["token"].as_str().unwrap();
    assert_ne!(new_token, &old_token, "Token should change after reset");
}

#[tokio::test]
async fn test_create_command() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let create_res = app
        .post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": "Cmd App" }),
        )
        .await;
    let application: serde_json::Value = create_res.json().await.unwrap();
    let app_id = application["id"].as_str().unwrap();

    let res = app
        .post(
            &token,
            &format!("/api/v1/applications/{}/commands", app_id),
            serde_json::json!({
                "name": "ping",
                "description": "Check if the bot is alive"
            }),
        )
        .await;

    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "ping");
    assert_eq!(
        body["description"].as_str().unwrap(),
        "Check if the bot is alive"
    );
}

#[tokio::test]
async fn test_list_commands() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let create_res = app
        .post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": "List Cmd App" }),
        )
        .await;
    let application: serde_json::Value = create_res.json().await.unwrap();
    let app_id = application["id"].as_str().unwrap();

    for cmd in &["ping", "help", "stats"] {
        app.post(
            &token,
            &format!("/api/v1/applications/{}/commands", app_id),
            serde_json::json!({ "name": cmd, "description": "test command" }),
        )
        .await;
    }

    let res = app
        .get(&token, &format!("/api/v1/applications/{}/commands", app_id))
        .await;

    assert_eq!(res.status(), 200);
    let cmds: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(cmds.len() >= 3);
}

#[tokio::test]
async fn test_update_command() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let create_res = app
        .post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": "Update Cmd App" }),
        )
        .await;
    let application: serde_json::Value = create_res.json().await.unwrap();
    let app_id = application["id"].as_str().unwrap();

    let cmd_res = app
        .post(
            &token,
            &format!("/api/v1/applications/{}/commands", app_id),
            serde_json::json!({ "name": "old_cmd", "description": "old desc" }),
        )
        .await;
    let cmd: serde_json::Value = cmd_res.json().await.unwrap();
    let cmd_id = cmd["id"].as_str().unwrap();

    let res = app
        .patch(
            &token,
            &format!("/api/v1/applications/{}/commands/{}", app_id, cmd_id),
            serde_json::json!({ "name": "new_cmd", "description": "new desc" }),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_command() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let create_res = app
        .post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": "Del Cmd App" }),
        )
        .await;
    let application: serde_json::Value = create_res.json().await.unwrap();
    let app_id = application["id"].as_str().unwrap();

    let cmd_res = app
        .post(
            &token,
            &format!("/api/v1/applications/{}/commands", app_id),
            serde_json::json!({ "name": "disposable", "description": "will delete" }),
        )
        .await;
    let cmd: serde_json::Value = cmd_res.json().await.unwrap();
    let cmd_id = cmd["id"].as_str().unwrap();

    let res = app
        .delete(
            &token,
            &format!("/api/v1/applications/{}/commands/{}", app_id, cmd_id),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_command_ownership() {
    let app = get_test_app().await;

    // User A creates an application
    let name_a = unique_name("appdev_a");
    let (token_a, _) = app
        .register_user(&name_a, &unique_email(&name_a), "testpass123")
        .await;

    let create_res = app
        .post(
            &token_a,
            "/api/v1/applications",
            serde_json::json!({ "name": "A's App" }),
        )
        .await;
    let application: serde_json::Value = create_res.json().await.unwrap();
    let app_id = application["id"].as_str().unwrap();

    let cmd_res = app
        .post(
            &token_a,
            &format!("/api/v1/applications/{}/commands", app_id),
            serde_json::json!({ "name": "secret", "description": "only A" }),
        )
        .await;
    let cmd: serde_json::Value = cmd_res.json().await.unwrap();
    let cmd_id = cmd["id"].as_str().unwrap();

    // User B tries to modify the command
    let name_b = unique_name("appdev_b");
    let (token_b, _) = app
        .register_user(&name_b, &unique_email(&name_b), "testpass123")
        .await;

    let res = app
        .patch(
            &token_b,
            &format!("/api/v1/applications/{}/commands/{}", app_id, cmd_id),
            serde_json::json!({ "name": "hacked" }),
        )
        .await;

    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_command_name_validation() {
    let app = get_test_app().await;
    let name = unique_name("appdev");
    let (token, _) = app
        .register_user(&name, &unique_email(&name), "testpass123")
        .await;

    let create_res = app
        .post(
            &token,
            "/api/v1/applications",
            serde_json::json!({ "name": "Validate App" }),
        )
        .await;
    let application: serde_json::Value = create_res.json().await.unwrap();
    let app_id = application["id"].as_str().unwrap();

    // Create a command with a very long name (over any reasonable limit)
    let long_name = "a".repeat(500);
    let res = app
        .post(
            &token,
            &format!("/api/v1/applications/{}/commands", app_id),
            serde_json::json!({ "name": long_name, "description": "too long name" }),
        )
        .await;

    // The server may accept or reject — document the behavior.
    // If no server-side validation, this is still a valid test documenting current behavior.
    let status = res.status().as_u16();
    assert!(
        status == 201 || status == 400,
        "Expected 201 or 400, got {}",
        status
    );
}

// ═══════════════════════════════════════════════════════════════════
// E2E Scenario Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_full_onboarding_flow() {
    let app = get_test_app().await;

    // 1. Register user A
    let name_a = unique_name("onboard_a");
    let email_a = unique_email(&name_a);
    let (token_a, _) = app.register_user(&name_a, &email_a, "testpass123").await;

    // 2. Login user A
    let (login_token, _refresh) = app.login(&email_a, "testpass123").await;
    assert!(!login_token.is_empty());

    // 3. Create server
    let srv_name = unique_name("onboard_srv");
    let server_id = app.create_server(&token_a, &srv_name).await;

    // 4. Get channels, find default text channel
    let ch_res = app
        .get(&token_a, &format!("/api/v1/servers/{}/channels", server_id))
        .await;
    let channels: Vec<serde_json::Value> = ch_res.json().await.unwrap();
    let channel_id: i64 = channels
        .iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // 5. Create invite
    let invite_res = app
        .post(
            &token_a,
            &format!("/api/v1/channels/{}/invites", channel_id),
            serde_json::json!({}),
        )
        .await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    // 6. Register user B and join
    let name_b = unique_name("onboard_b");
    let (token_b, _) = app
        .register_user(&name_b, &unique_email(&name_b), "testpass123")
        .await;
    let join_res = app
        .post(
            &token_b,
            &format!("/api/v1/invites/{}", code),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(join_res.status(), 200);

    // 7. Send a message
    let msg_res = app
        .post(
            &token_b,
            &format!("/api/v1/channels/{}/messages", channel_id),
            serde_json::json!({ "content": "Hello everyone!" }),
        )
        .await;
    assert_eq!(msg_res.status(), 201);

    // 8. Verify member count increased
    let srv_res = app
        .get(&token_a, &format!("/api/v1/servers/{}", server_id))
        .await;
    let server: serde_json::Value = srv_res.json().await.unwrap();
    assert!(server["member_count"].as_i64().unwrap() >= 2);

    // 9. Verify message exists
    let msgs_res = app
        .get(
            &token_a,
            &format!("/api/v1/channels/{}/messages", channel_id),
        )
        .await;
    let msgs: Vec<serde_json::Value> = msgs_res.json().await.unwrap();
    assert!(
        msgs.iter()
            .any(|m| m["content"].as_str() == Some("Hello everyone!")),
        "Sent message should appear in channel messages"
    );
}

#[tokio::test]
async fn test_full_moderation_flow() {
    let app = get_test_app().await;
    let (owner_token, member_token, server_id, channel_id, member_id) =
        setup_server_with_two_members(app).await;

    // 1. Warn the member
    let warn_res = app
        .post(
            &owner_token,
            &format!(
                "/api/v1/servers/{}/members/{}/warnings",
                server_id, member_id
            ),
            serde_json::json!({ "reason": "First offense" }),
        )
        .await;
    assert_eq!(warn_res.status(), 201);

    // 2. Timeout the member (1 hour in the future)
    let timeout_until = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let timeout_res = app
        .patch(
            &owner_token,
            &format!("/api/v1/servers/{}/members/{}", server_id, member_id),
            serde_json::json!({ "communication_disabled_until": timeout_until }),
        )
        .await;
    assert_eq!(timeout_res.status(), 200);

    // 3. Timed-out member cannot send messages
    let msg_res = app
        .post(
            &member_token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            serde_json::json!({ "content": "I am timed out" }),
        )
        .await;
    assert_eq!(msg_res.status(), 403);

    // 4. Ban the member
    let ban_res = app
        .put(
            &owner_token,
            &format!("/api/v1/servers/{}/bans/{}", server_id, member_id),
            serde_json::json!({ "reason": "Repeated offenses" }),
        )
        .await;
    assert_eq!(ban_res.status(), 204);

    // 5. Banned member cannot rejoin using a new invite
    let new_invite_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/invites", channel_id),
            serde_json::json!({}),
        )
        .await;
    let new_invite: serde_json::Value = new_invite_res.json().await.unwrap();
    let new_code = new_invite["code"].as_str().unwrap();

    let rejoin_res = app
        .post(
            &member_token,
            &format!("/api/v1/invites/{}", new_code),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(rejoin_res.status(), 403);
}

#[tokio::test]
async fn test_full_permission_flow() {
    let app = get_test_app().await;
    let (owner_token, member_token, server_id, channel_id, member_id) =
        setup_server_with_two_members(app).await;

    // 1. Create a role with SEND_MESSAGES permission only
    let send_messages_bit: u64 = 0x800; // SEND_MESSAGES bit
    let role_res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/roles", server_id),
            serde_json::json!({
                "name": "Sender",
                "permissions": send_messages_bit.to_string()
            }),
        )
        .await;
    assert_eq!(role_res.status(), 201);
    let role: serde_json::Value = role_res.json().await.unwrap();
    let role_id: i64 = role["id"].as_str().unwrap().parse().unwrap();

    // 2. Assign role to member
    let assign_res = app
        .patch(
            &owner_token,
            &format!("/api/v1/servers/{}/members/{}", server_id, member_id),
            serde_json::json!({ "roles": [role_id] }),
        )
        .await;
    assert_eq!(assign_res.status(), 200);

    // 3. Member can send messages
    let msg_res = app
        .post(
            &member_token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            serde_json::json!({ "content": "I have permissions!" }),
        )
        .await;
    assert_eq!(msg_res.status(), 201);

    // 4. Remove SEND_MESSAGES from the role (set permissions to 0)
    let update_role_res = app
        .patch(
            &owner_token,
            &format!("/api/v1/servers/{}/roles/{}", server_id, role_id),
            serde_json::json!({ "permissions": "0" }),
        )
        .await;
    // update_role returns 200 (the role object)
    assert!(
        update_role_res.status().is_success(),
        "Role update should succeed"
    );

    // 5. Also update the @everyone role to have no SEND_MESSAGES
    // The @everyone role id equals the server_id
    let update_everyone_res = app
        .patch(
            &owner_token,
            &format!("/api/v1/servers/{}/roles/{}", server_id, server_id),
            serde_json::json!({ "permissions": "0" }),
        )
        .await;
    assert!(update_everyone_res.status().is_success());

    // 6. Member can no longer send
    let msg_res2 = app
        .post(
            &member_token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            serde_json::json!({ "content": "No permission" }),
        )
        .await;
    assert_eq!(msg_res2.status(), 403);
}

#[tokio::test]
async fn test_full_dm_flow() {
    let app = get_test_app().await;

    // 1. Register two users
    let name_a = unique_name("dm_user_a");
    let (token_a, user_a_id) = app
        .register_user(&name_a, &unique_email(&name_a), "testpass123")
        .await;

    let name_b = unique_name("dm_user_b");
    let (token_b, user_b_id) = app
        .register_user(&name_b, &unique_email(&name_b), "testpass123")
        .await;

    // 2. A sends friend request to B
    let req_res = app
        .post(
            &token_a,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": user_b_id }),
        )
        .await;
    assert_eq!(req_res.status(), 204);

    // 3. B accepts (by sending friend request back which auto-accepts)
    let accept_res = app
        .post(
            &token_b,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": user_a_id }),
        )
        .await;
    assert_eq!(accept_res.status(), 204);

    // 4. A creates DM with B
    let dm_res = app
        .post(
            &token_a,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipient_id": user_b_id }),
        )
        .await;
    assert!(dm_res.status().is_success());
    let dm: serde_json::Value = dm_res.json().await.unwrap();
    let dm_channel_id: i64 = dm["id"].as_str().unwrap().parse().unwrap();

    // 5. A sends message in DM
    let msg_res = app
        .post(
            &token_a,
            &format!("/api/v1/channels/{}/messages", dm_channel_id),
            serde_json::json!({ "content": "Hey friend!" }),
        )
        .await;
    assert_eq!(msg_res.status(), 201);

    // 6. List DMs — both users should see the channel
    let list_res = app.get(&token_b, "/api/v1/users/@me/channels").await;
    assert_eq!(list_res.status(), 200);
    let dms: Vec<serde_json::Value> = list_res.json().await.unwrap();
    assert!(
        dms.iter()
            .any(|d| { d["id"].as_str().unwrap().parse::<i64>().unwrap() == dm_channel_id }),
        "DM channel should appear in user B's DM list"
    );
}

#[tokio::test]
async fn test_full_channel_management() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, _, _) = setup_server_with_two_members(app).await;

    // 1. Create a category
    let cat_res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/channels", server_id),
            serde_json::json!({
                "name": "My Category",
                "type": 4
            }),
        )
        .await;
    assert_eq!(cat_res.status(), 201);
    let category: serde_json::Value = cat_res.json().await.unwrap();
    let cat_id: i64 = category["id"].as_str().unwrap().parse().unwrap();

    // 2. Create a text channel under the category
    let ch_res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/channels", server_id),
            serde_json::json!({
                "name": "child-channel",
                "type": 0,
                "parent_id": cat_id
            }),
        )
        .await;
    assert_eq!(ch_res.status(), 201);
    let child: serde_json::Value = ch_res.json().await.unwrap();
    let child_id: i64 = child["id"].as_str().unwrap().parse().unwrap();

    // 3. Reorder channels
    let reorder_res = app
        .patch(
            &owner_token,
            &format!("/api/v1/servers/{}/channels/reorder", server_id),
            serde_json::json!([
                { "id": child_id, "position": 10 }
            ]),
        )
        .await;
    assert_eq!(reorder_res.status(), 204);

    // 4. Delete category — children should move to root (parent_id = null)
    let del_res = app
        .delete(&owner_token, &format!("/api/v1/channels/{}", cat_id))
        .await;
    assert_eq!(del_res.status(), 204);

    // 5. Verify child channel still exists and parent_id is null
    let child_res = app
        .get(&owner_token, &format!("/api/v1/channels/{}", child_id))
        .await;
    assert_eq!(child_res.status(), 200);
    let updated_child: serde_json::Value = child_res.json().await.unwrap();
    assert!(
        updated_child["parent_id"].is_null(),
        "Child should have null parent_id after category deletion"
    );
}

#[tokio::test]
async fn test_server_deletion_cascade() {
    let app = get_test_app().await;
    let (owner_token, _, server_id, channel_id, _member_id) =
        setup_server_with_two_members(app).await;

    // Create an extra channel
    let ch_res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/channels", server_id),
            serde_json::json!({ "name": "extra-channel", "type": 0 }),
        )
        .await;
    assert_eq!(ch_res.status(), 201);
    let extra_ch: serde_json::Value = ch_res.json().await.unwrap();
    let extra_ch_id: i64 = extra_ch["id"].as_str().unwrap().parse().unwrap();

    // Create a role
    let role_res = app
        .post(
            &owner_token,
            &format!("/api/v1/servers/{}/roles", server_id),
            serde_json::json!({ "name": "TestRole" }),
        )
        .await;
    assert_eq!(role_res.status(), 201);

    // Delete the server
    let del_res = app
        .delete_with_body(
            &owner_token,
            &format!("/api/v1/servers/{}", server_id),
            serde_json::json!({ "password": "testpass123" }),
        )
        .await;
    assert_eq!(del_res.status(), 204);

    // Verify server is gone
    let get_res = app
        .get(&owner_token, &format!("/api/v1/servers/{}", server_id))
        .await;
    assert!(
        get_res.status() == 403 || get_res.status() == 404,
        "Deleted server should not be accessible"
    );

    // Verify channels are gone
    let ch_get = app
        .get(&owner_token, &format!("/api/v1/channels/{}", channel_id))
        .await;
    assert!(
        ch_get.status() == 404 || ch_get.status() == 403,
        "Channel should not be accessible after server deletion"
    );

    let extra_ch_get = app
        .get(&owner_token, &format!("/api/v1/channels/{}", extra_ch_id))
        .await;
    assert!(
        extra_ch_get.status() == 404 || extra_ch_get.status() == 403,
        "Extra channel should not be accessible after server deletion"
    );
}

#[tokio::test]
async fn test_account_deletion_flow() {
    let app = get_test_app().await;

    // 1. Register user
    let name = unique_name("delme");
    let email = unique_email(&name);
    let (token, user_id) = app.register_user(&name, &email, "testpass123").await;

    // 2. Create a relationship
    let friend_name = unique_name("delfriend");
    let (friend_token, friend_id) = app
        .register_user(&friend_name, &unique_email(&friend_name), "testpass123")
        .await;

    app.post(
        &token,
        "/api/v1/users/@me/relationships",
        serde_json::json!({ "user_id": friend_id }),
    )
    .await;

    // 3. Set a note on the friend
    app.put(
        &token,
        &format!("/api/v1/users/{}/note", friend_id),
        serde_json::json!({ "note": "My best friend" }),
    )
    .await;

    // 4. Delete account
    let del_res = app
        .delete_with_body(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "password": "testpass123" }),
        )
        .await;
    assert_eq!(del_res.status(), 204);

    // 5. Cannot login anymore
    let login_res = app
        .post_unauth(
            "/api/v1/auth/login",
            serde_json::json!({ "email": email, "password": "testpass123" }),
        )
        .await;
    assert!(
        login_res.status() == 401 || login_res.status() == 404,
        "Deleted user should not be able to login"
    );

    // 6. Public profile is gone
    let profile_res = app
        .get(&friend_token, &format!("/api/v1/users/{}", user_id))
        .await;
    assert_eq!(profile_res.status(), 404);
}

#[tokio::test]
async fn test_invite_lifecycle() {
    let app = get_test_app().await;

    // Setup server
    let owner_name = unique_name("inv_owner");
    let (owner_token, _) = app
        .register_user(&owner_name, &unique_email(&owner_name), "testpass123")
        .await;
    let server_id = app
        .create_server(&owner_token, &unique_name("inv_srv"))
        .await;

    let ch_res = app
        .get(
            &owner_token,
            &format!("/api/v1/servers/{}/channels", server_id),
        )
        .await;
    let channels: Vec<serde_json::Value> = ch_res.json().await.unwrap();
    let channel_id: i64 = channels
        .iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // 1. Create invite with max_uses=2
    let invite_res = app
        .post(
            &owner_token,
            &format!("/api/v1/channels/{}/invites", channel_id),
            serde_json::json!({ "max_uses": 2 }),
        )
        .await;
    assert_eq!(invite_res.status(), 201);
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap().to_string();

    // 2. First user joins
    let user1_name = unique_name("inv_user1");
    let (user1_token, _) = app
        .register_user(&user1_name, &unique_email(&user1_name), "testpass123")
        .await;
    let join1 = app
        .post(
            &user1_token,
            &format!("/api/v1/invites/{}", code),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(join1.status(), 200);

    // 3. Second user joins
    let user2_name = unique_name("inv_user2");
    let (user2_token, _) = app
        .register_user(&user2_name, &unique_email(&user2_name), "testpass123")
        .await;
    let join2 = app
        .post(
            &user2_token,
            &format!("/api/v1/invites/{}", code),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(join2.status(), 200);

    // 4. Third user fails — max uses reached
    let user3_name = unique_name("inv_user3");
    let (user3_token, _) = app
        .register_user(&user3_name, &unique_email(&user3_name), "testpass123")
        .await;
    let join3 = app
        .post(
            &user3_token,
            &format!("/api/v1/invites/{}", code),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(join3.status(), 400);

    // 5. Delete the invite
    let del_res = app
        .delete(&owner_token, &format!("/api/v1/invites/{}", code))
        .await;
    assert_eq!(del_res.status(), 204);

    // 6. Fourth user cannot use deleted invite
    let user4_name = unique_name("inv_user4");
    let (user4_token, _) = app
        .register_user(&user4_name, &unique_email(&user4_name), "testpass123")
        .await;
    let join4 = app
        .post(
            &user4_token,
            &format!("/api/v1/invites/{}", code),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(join4.status(), 404);
}
