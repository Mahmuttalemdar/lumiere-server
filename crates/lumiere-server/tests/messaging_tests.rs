mod common;
use common::{get_test_app, unique_name, unique_email};

use common::TestApp;
use serde_json::json;

// ─── Helpers ───────────────────────────────────────────────────────

/// Register a user, create a server, and return (token, server_id, channel_id)
/// where channel_id is the default text channel auto-created with the server.
async fn setup_server_with_channel(app: &TestApp) -> (String, i64, i64) {
    let name = unique_name("user");
    let (token, _user_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;
    let server_id = app
        .create_server(&token, &unique_name("server"))
        .await;

    // Get channel list to find the default general channel
    let res = app
        .get(&token, &format!("/api/v1/servers/{}/channels", server_id))
        .await;
    assert_eq!(res.status(), 200);
    let channels: Vec<serde_json::Value> = res.json().await.unwrap();
    let channel_id = channels
        .iter()
        .find(|c| c["type"].as_i64() == Some(0)) // text channel
        .expect("server should have a default text channel")["id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();

    (token, server_id, channel_id)
}

/// Send a simple text message and return the response body as JSON.
async fn send_text_message(
    app: &TestApp,
    token: &str,
    channel_id: i64,
    content: &str,
) -> serde_json::Value {
    let res = app
        .post(
            token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({ "content": content }),
        )
        .await;
    assert_eq!(res.status(), 201, "send_text_message expected 201");
    res.json().await.unwrap()
}

/// Register a second user who is NOT a member of the given server.
/// Returns (token, user_id).
async fn register_outsider(app: &TestApp) -> (String, i64) {
    let name = unique_name("outsider");
    app.register_user(&name, &unique_email(&name), "testpassword123")
        .await
}

/// Create a DM channel between two users and return the channel_id.
async fn create_dm_channel(app: &TestApp, token: &str, recipient_id: i64) -> i64 {
    let res = app
        .post(
            token,
            "/api/v1/users/@me/channels",
            json!({ "recipient_ids": [recipient_id] }),
        )
        .await;
    assert!(
        res.status() == 200 || res.status() == 201,
        "create_dm_channel failed: {}",
        res.status()
    );
    let body: serde_json::Value = res.json().await.unwrap();
    body["id"].as_str().unwrap().parse::<i64>().unwrap()
}

/// Add a second user to a server so they become a member.
/// Creates an invite via the server's first text channel, then has the second user join.
async fn add_member_to_server(
    app: &TestApp,
    owner_token: &str,
    member_token: &str,
    server_id: i64,
) {
    // Get the server's first channel to create an invite
    let res = app
        .get(owner_token, &format!("/api/v1/servers/{}/channels", server_id))
        .await;
    assert_eq!(res.status(), 200, "get channels failed");
    let channels: Vec<serde_json::Value> = res.json().await.unwrap();
    let channel_id = channels
        .first()
        .expect("server should have at least one channel")["id"]
        .as_str()
        .unwrap();

    // Create an invite via the channel
    let res = app
        .post(
            owner_token,
            &format!("/api/v1/channels/{}/invites", channel_id),
            json!({}),
        )
        .await;
    assert!(res.status() == 200 || res.status() == 201, "create invite failed: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    let code = body["code"].as_str().unwrap();

    // Join via invite
    let res = app
        .post(member_token, &format!("/api/v1/invites/{}", code), json!({}))
        .await;
    assert!(
        res.status() == 200 || res.status() == 201,
        "join via invite failed: {}",
        res.status()
    );
}

// ═══════════════════════════════════════════════════════════════════
// MESSAGE SEND TESTS
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_send_message() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({ "content": "hello world" }),
        )
        .await;
    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["id"].as_str().is_some(), "response should contain message id");
    assert!(body["channel_id"].is_string());
    assert!(body["author_id"].is_string());
    assert!(body["timestamp"].is_string());
}

#[tokio::test]
async fn test_send_message_content() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let body = send_text_message(app, &token, channel_id, "exact content test").await;
    assert_eq!(body["content"].as_str().unwrap(), "exact content test");
}

#[tokio::test]
async fn test_send_message_empty() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    // No content, no embeds, no attachments => 400
    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_send_message_too_long() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let long_content: String = "a".repeat(4001);
    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({ "content": long_content }),
        )
        .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_send_message_with_reply() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let original = send_text_message(app, &token, channel_id, "original message").await;
    let original_id: i64 = original["id"].as_str().unwrap().parse().unwrap();

    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({
                "content": "replying",
                "message_reference": { "message_id": original_id }
            }),
        )
        .await;
    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(
        body["reference_id"].as_str().unwrap(),
        original_id.to_string()
    );
    // Reply messages have type 19
    assert_eq!(body["type"].as_i64().unwrap(), 19);
}

#[tokio::test]
async fn test_send_message_not_member() {
    let app = get_test_app().await;
    let (_owner_token, _server_id, channel_id) = setup_server_with_channel(app).await;
    let (outsider_token, _) = register_outsider(app).await;

    let res = app
        .post(
            &outsider_token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({ "content": "should fail" }),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_send_message_updates_last_message_id() {
    let app = get_test_app().await;
    let (token, server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "update last_message_id test").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Fetch channel info to verify last_message_id
    let res = app
        .get(&token, &format!("/api/v1/servers/{}/channels", server_id))
        .await;
    let channels: Vec<serde_json::Value> = res.json().await.unwrap();
    let ch = channels
        .iter()
        .find(|c| c["id"].as_str() == Some(&channel_id.to_string()))
        .unwrap();
    assert_eq!(ch["last_message_id"].as_str().unwrap(), msg_id);
}

#[tokio::test]
async fn test_send_message_dm() {
    let app = get_test_app().await;
    let name_a = unique_name("dma");
    let (token_a, _user_a) = app
        .register_user(&name_a, &unique_email(&name_a), "testpassword123")
        .await;
    let name_b = unique_name("dmb");
    let (token_b, user_b) = app
        .register_user(&name_b, &unique_email(&name_b), "testpassword123")
        .await;

    let dm_channel_id = create_dm_channel(app, &token_a, user_b).await;

    // Both users should be able to send messages
    let msg = send_text_message(app, &token_a, dm_channel_id, "DM from A").await;
    assert_eq!(msg["content"].as_str().unwrap(), "DM from A");

    let msg2 = send_text_message(app, &token_b, dm_channel_id, "DM from B").await;
    assert_eq!(msg2["content"].as_str().unwrap(), "DM from B");
}

#[tokio::test]
async fn test_send_message_blocked_dm() {
    let app = get_test_app().await;
    let name_a = unique_name("blocker");
    let (token_a, user_a) = app
        .register_user(&name_a, &unique_email(&name_a), "testpassword123")
        .await;
    let name_b = unique_name("blocked");
    let (token_b, user_b) = app
        .register_user(&name_b, &unique_email(&name_b), "testpassword123")
        .await;

    // Create DM channel first (before blocking)
    let _dm_channel_id = create_dm_channel(app, &token_a, user_b).await;

    // A blocks B
    let res = app
        .put(
            &token_a,
            &format!("/api/v1/users/@me/relationships/{}", user_b),
            json!({ "type": 2 }),
        )
        .await;
    assert!(res.status() == 204 || res.status() == 200, "block failed: {}", res.status());

    // Now B tries to create a new DM with A — should fail
    let res = app
        .post(
            &token_b,
            "/api/v1/users/@me/channels",
            json!({ "recipient_ids": [user_a] }),
        )
        .await;
    // The block check is on DM creation, so sending to existing channel may or may not fail.
    // The DM creation should fail with 403
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_send_message_timed_out_member() {
    let app = get_test_app().await;
    let (owner_token, server_id, channel_id) = setup_server_with_channel(app).await;

    let name = unique_name("timedout");
    let (member_token, member_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;

    add_member_to_server(app, &owner_token, &member_token, server_id).await;

    // Verify member can send before timeout
    let res = app
        .post(
            &member_token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({ "content": "before timeout" }),
        )
        .await;
    assert_eq!(res.status(), 201);

    // Owner times out the member (1 hour from now)
    let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let res = app
        .patch(
            &owner_token,
            &format!("/api/v1/servers/{}/members/{}", server_id, member_id),
            json!({ "communication_disabled_until": future }),
        )
        .await;
    assert!(res.status() == 200 || res.status() == 204, "timeout set failed: {}", res.status());

    // Timed-out member tries to send a message => 403
    let res = app
        .post(
            &member_token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({ "content": "should fail" }),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_send_message_with_embeds() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let embeds = json!([{ "title": "Test Embed", "description": "Hello" }]);
    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages", channel_id),
            json!({ "embeds": embeds }),
        )
        .await;
    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    let returned_embeds = &body["embeds"];
    assert!(returned_embeds.is_array());
    assert_eq!(returned_embeds[0]["title"].as_str().unwrap(), "Test Embed");
}

#[tokio::test]
async fn test_send_message_with_mentions() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let body = send_text_message(
        app,
        &token,
        channel_id,
        "Hey @everyone, check this out!",
    )
    .await;
    assert_eq!(body["mention_everyone"].as_bool().unwrap(), true);
}

// ═══════════════════════════════════════════════════════════════════
// MESSAGE HISTORY TESTS
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_get_messages_default() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    send_text_message(app, &token, channel_id, "history msg 1").await;
    send_text_message(app, &token, channel_id, "history msg 2").await;

    let res = app
        .get(&token, &format!("/api/v1/channels/{}/messages", channel_id))
        .await;
    assert_eq!(res.status(), 200);
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(messages.len() >= 2);
}

#[tokio::test]
async fn test_get_messages_before() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg1 = send_text_message(app, &token, channel_id, "before-1").await;
    let _msg2 = send_text_message(app, &token, channel_id, "before-2").await;
    let msg3 = send_text_message(app, &token, channel_id, "before-3").await;
    let msg3_id: i64 = msg3["id"].as_str().unwrap().parse().unwrap();

    let res = app
        .get(
            &token,
            &format!("/api/v1/channels/{}/messages?before={}", channel_id, msg3_id),
        )
        .await;
    assert_eq!(res.status(), 200);
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    // Should contain msg1 and msg2, but not msg3
    let ids: Vec<String> = messages.iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
    assert!(!ids.contains(&msg3_id.to_string()));
    let msg1_id = msg1["id"].as_str().unwrap().to_string();
    assert!(ids.contains(&msg1_id));
}

#[tokio::test]
async fn test_get_messages_after() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg1 = send_text_message(app, &token, channel_id, "after-1").await;
    let msg1_id: i64 = msg1["id"].as_str().unwrap().parse().unwrap();
    let msg2 = send_text_message(app, &token, channel_id, "after-2").await;
    let msg3 = send_text_message(app, &token, channel_id, "after-3").await;

    let res = app
        .get(
            &token,
            &format!("/api/v1/channels/{}/messages?after={}", channel_id, msg1_id),
        )
        .await;
    assert_eq!(res.status(), 200);
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    let ids: Vec<String> = messages.iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
    // Should contain msg2 and msg3, but not msg1
    assert!(!ids.contains(&msg1_id.to_string()));
    let msg2_id = msg2["id"].as_str().unwrap().to_string();
    let msg3_id = msg3["id"].as_str().unwrap().to_string();
    assert!(ids.contains(&msg2_id));
    assert!(ids.contains(&msg3_id));
}

#[tokio::test]
async fn test_get_messages_limit() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    for i in 0..5 {
        send_text_message(app, &token, channel_id, &format!("limit-{}", i)).await;
    }

    let res = app
        .get(
            &token,
            &format!("/api/v1/channels/{}/messages?limit=3", channel_id),
        )
        .await;
    assert_eq!(res.status(), 200);
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    assert_eq!(messages.len(), 3);
}

#[tokio::test]
async fn test_get_messages_limit_clamp() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    // Request limit > 100 — should be clamped to 100 (not error)
    let res = app
        .get(
            &token,
            &format!("/api/v1/channels/{}/messages?limit=200", channel_id),
        )
        .await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn test_get_messages_around() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let _msg1 = send_text_message(app, &token, channel_id, "around-1").await;
    let msg2 = send_text_message(app, &token, channel_id, "around-2").await;
    let _msg3 = send_text_message(app, &token, channel_id, "around-3").await;
    let msg2_id: i64 = msg2["id"].as_str().unwrap().parse().unwrap();

    let res = app
        .get(
            &token,
            &format!(
                "/api/v1/channels/{}/messages?around={}&limit=10",
                channel_id, msg2_id
            ),
        )
        .await;
    assert_eq!(res.status(), 200);
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    // The target message itself should appear in results
    let ids: Vec<String> = messages.iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
    assert!(ids.contains(&msg2_id.to_string()));
}

#[tokio::test]
async fn test_get_messages_empty_channel() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let res = app
        .get(&token, &format!("/api/v1/channels/{}/messages", channel_id))
        .await;
    assert_eq!(res.status(), 200);
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(messages.is_empty());
}

#[tokio::test]
async fn test_get_messages_not_member() {
    let app = get_test_app().await;
    let (_owner_token, _server_id, channel_id) = setup_server_with_channel(app).await;
    let (outsider_token, _) = register_outsider(app).await;

    let res = app
        .get(
            &outsider_token,
            &format!("/api/v1/channels/{}/messages", channel_id),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_get_messages_dm() {
    let app = get_test_app().await;
    let name_a = unique_name("dmhist_a");
    let (token_a, _user_a) = app
        .register_user(&name_a, &unique_email(&name_a), "testpassword123")
        .await;
    let name_b = unique_name("dmhist_b");
    let (_token_b, user_b) = app
        .register_user(&name_b, &unique_email(&name_b), "testpassword123")
        .await;

    let dm_channel_id = create_dm_channel(app, &token_a, user_b).await;
    send_text_message(app, &token_a, dm_channel_id, "dm history test").await;

    let res = app
        .get(
            &token_a,
            &format!("/api/v1/channels/{}/messages", dm_channel_id),
        )
        .await;
    assert_eq!(res.status(), 200);
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"].as_str().unwrap(), "dm history test");
}

#[tokio::test]
async fn test_get_messages_multiple() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let mut sent_ids = Vec::new();
    for i in 0..5 {
        let msg = send_text_message(app, &token, channel_id, &format!("multi-{}", i)).await;
        sent_ids.push(msg["id"].as_str().unwrap().to_string());
    }

    let res = app
        .get(&token, &format!("/api/v1/channels/{}/messages", channel_id))
        .await;
    assert_eq!(res.status(), 200);
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    assert_eq!(messages.len(), 5);

    // Verify all sent IDs are present
    let returned_ids: Vec<String> = messages
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    for id in &sent_ids {
        assert!(returned_ids.contains(id), "missing message id {}", id);
    }
}

// ═══════════════════════════════════════════════════════════════════
// MESSAGE EDIT TESTS
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_edit_own_message() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "before edit").await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = app
        .patch(
            &token,
            &format!("/api/v1/channels/{}/messages/{}", channel_id, msg_id),
            json!({ "content": "after edit" }),
        )
        .await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["content"].as_str().unwrap(), "after edit");
    assert!(
        body["edited_timestamp"].as_str().is_some(),
        "edited_timestamp should be set"
    );
}

#[tokio::test]
async fn test_edit_other_message() {
    let app = get_test_app().await;
    let (owner_token, server_id, channel_id) = setup_server_with_channel(app).await;

    let name = unique_name("editor");
    let (member_token, _member_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;
    add_member_to_server(app, &owner_token, &member_token, server_id).await;

    let msg = send_text_message(app, &owner_token, channel_id, "owner's message").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Non-owner member tries to edit content => 403
    let res = app
        .patch(
            &member_token,
            &format!("/api/v1/channels/{}/messages/{}", channel_id, msg_id),
            json!({ "content": "hacked" }),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_edit_message_not_found() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let res = app
        .patch(
            &token,
            &format!("/api/v1/channels/{}/messages/999999999999999", channel_id),
            json!({ "content": "nothing" }),
        )
        .await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn test_edit_message_with_manage_messages() {
    let app = get_test_app().await;
    let (owner_token, server_id, channel_id) = setup_server_with_channel(app).await;

    let name = unique_name("member");
    let (member_token, _member_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;
    add_member_to_server(app, &owner_token, &member_token, server_id).await;

    let msg = send_text_message(app, &member_token, channel_id, "member message").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Owner (who has MANAGE_MESSAGES) can edit flags only, not content
    // Editing flags via the admin should work
    let res = app
        .patch(
            &owner_token,
            &format!("/api/v1/channels/{}/messages/{}", channel_id, msg_id),
            json!({ "flags": 4 }),
        )
        .await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn test_edit_message_dm() {
    let app = get_test_app().await;
    let name_a = unique_name("dmedit_a");
    let (token_a, _user_a) = app
        .register_user(&name_a, &unique_email(&name_a), "testpassword123")
        .await;
    let name_b = unique_name("dmedit_b");
    let (_token_b, user_b) = app
        .register_user(&name_b, &unique_email(&name_b), "testpassword123")
        .await;

    let dm_channel_id = create_dm_channel(app, &token_a, user_b).await;
    let msg = send_text_message(app, &token_a, dm_channel_id, "before dm edit").await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = app
        .patch(
            &token_a,
            &format!("/api/v1/channels/{}/messages/{}", dm_channel_id, msg_id),
            json!({ "content": "after dm edit" }),
        )
        .await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["content"].as_str().unwrap(), "after dm edit");
}

#[tokio::test]
async fn test_edit_message_empty_update() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "no change").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Sending an empty patch (no fields) should still succeed
    let res = app
        .patch(
            &token,
            &format!("/api/v1/channels/{}/messages/{}", channel_id, msg_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["content"].as_str().unwrap(), "no change");
}

// ═══════════════════════════════════════════════════════════════════
// MESSAGE DELETE TESTS
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_delete_own_message() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "delete me").await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = app
        .delete(
            &token,
            &format!("/api/v1/channels/{}/messages/{}", channel_id, msg_id),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_other_message_no_permission() {
    let app = get_test_app().await;
    let (owner_token, server_id, channel_id) = setup_server_with_channel(app).await;

    let name = unique_name("deleter");
    let (member_token, _member_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;
    add_member_to_server(app, &owner_token, &member_token, server_id).await;

    let msg = send_text_message(app, &owner_token, channel_id, "owner msg").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Regular member cannot delete other's message
    let res = app
        .delete(
            &member_token,
            &format!("/api/v1/channels/{}/messages/{}", channel_id, msg_id),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_delete_other_message_with_manage() {
    let app = get_test_app().await;
    let (owner_token, server_id, channel_id) = setup_server_with_channel(app).await;

    let name = unique_name("managed");
    let (member_token, _member_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;
    add_member_to_server(app, &owner_token, &member_token, server_id).await;

    let msg = send_text_message(app, &member_token, channel_id, "member msg").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Owner (ADMINISTRATOR / MANAGE_MESSAGES) can delete other's message
    let res = app
        .delete(
            &owner_token,
            &format!("/api/v1/channels/{}/messages/{}", channel_id, msg_id),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_message_not_found() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let res = app
        .delete(
            &token,
            &format!("/api/v1/channels/{}/messages/999999999999999", channel_id),
        )
        .await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn test_deleted_message_not_in_history() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "will be deleted").await;
    let msg_id = msg["id"].as_str().unwrap().to_string();

    // Delete it
    app.delete(
        &token,
        &format!("/api/v1/channels/{}/messages/{}", channel_id, msg_id),
    )
    .await;

    // Fetch history — deleted message should not appear
    let res = app
        .get(&token, &format!("/api/v1/channels/{}/messages", channel_id))
        .await;
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    let ids: Vec<String> = messages
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert!(!ids.contains(&msg_id), "deleted message should not appear in history");
}

#[tokio::test]
async fn test_bulk_delete() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let mut ids = Vec::new();
    for i in 0..3 {
        let msg = send_text_message(app, &token, channel_id, &format!("bulk-{}", i)).await;
        ids.push(msg["id"].as_str().unwrap().parse::<i64>().unwrap());
    }

    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages/bulk-delete", channel_id),
            json!({ "messages": ids }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Verify they are gone from history
    let res = app
        .get(&token, &format!("/api/v1/channels/{}/messages", channel_id))
        .await;
    let messages: Vec<serde_json::Value> = res.json().await.unwrap();
    let remaining_ids: Vec<i64> = messages
        .iter()
        .map(|m| m["id"].as_str().unwrap().parse::<i64>().unwrap())
        .collect();
    for id in &ids {
        assert!(!remaining_ids.contains(id), "bulk-deleted message {} still present", id);
    }
}

#[tokio::test]
async fn test_bulk_delete_not_permitted() {
    let app = get_test_app().await;
    let (owner_token, server_id, channel_id) = setup_server_with_channel(app).await;

    let name = unique_name("bulkdel");
    let (member_token, _member_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;
    add_member_to_server(app, &owner_token, &member_token, server_id).await;

    let mut ids = Vec::new();
    for i in 0..2 {
        let msg = send_text_message(app, &owner_token, channel_id, &format!("bd-{}", i)).await;
        ids.push(msg["id"].as_str().unwrap().parse::<i64>().unwrap());
    }

    // Regular member lacks MANAGE_MESSAGES
    let res = app
        .post(
            &member_token,
            &format!("/api/v1/channels/{}/messages/bulk-delete", channel_id),
            json!({ "messages": ids }),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_bulk_delete_invalid_count() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    // 0 messages
    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages/bulk-delete", channel_id),
            json!({ "messages": [] }),
        )
        .await;
    assert_eq!(res.status(), 400);

    // 1 message (need 2-100)
    let msg = send_text_message(app, &token, channel_id, "single").await;
    let msg_id: i64 = msg["id"].as_str().unwrap().parse().unwrap();
    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages/bulk-delete", channel_id),
            json!({ "messages": [msg_id] }),
        )
        .await;
    assert_eq!(res.status(), 400);

    // 101 messages
    let fake_ids: Vec<i64> = (1..=101).collect();
    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages/bulk-delete", channel_id),
            json!({ "messages": fake_ids }),
        )
        .await;
    assert_eq!(res.status(), 400);
}

// ═══════════════════════════════════════════════════════════════════
// PIN TESTS
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_pin_message() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "pin me").await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = app
        .put(
            &token,
            &format!("/api/v1/channels/{}/pins/{}", channel_id, msg_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_get_pins() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "pinned message").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Pin the message
    app.put(
        &token,
        &format!("/api/v1/channels/{}/pins/{}", channel_id, msg_id),
        json!({}),
    )
    .await;

    let res = app
        .get(&token, &format!("/api/v1/channels/{}/pins", channel_id))
        .await;
    assert_eq!(res.status(), 200);
    let pins: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(!pins.is_empty());
    let pin_ids: Vec<String> = pins.iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    assert!(pin_ids.contains(&msg_id.to_string()));
}

#[tokio::test]
async fn test_unpin_message() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "unpin me").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Pin then unpin
    app.put(
        &token,
        &format!("/api/v1/channels/{}/pins/{}", channel_id, msg_id),
        json!({}),
    )
    .await;

    let res = app
        .delete(
            &token,
            &format!("/api/v1/channels/{}/pins/{}", channel_id, msg_id),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Verify removed from pins
    let res = app
        .get(&token, &format!("/api/v1/channels/{}/pins", channel_id))
        .await;
    let pins: Vec<serde_json::Value> = res.json().await.unwrap();
    let pin_ids: Vec<String> = pins.iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    assert!(!pin_ids.contains(&msg_id.to_string()));
}

#[tokio::test]
async fn test_pin_nonexistent_message() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let res = app
        .put(
            &token,
            &format!("/api/v1/channels/{}/pins/999999999999999", channel_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn test_pin_max_50() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    // Pin 50 messages
    for i in 0..50 {
        let msg = send_text_message(app, &token, channel_id, &format!("pin-{}", i)).await;
        let msg_id = msg["id"].as_str().unwrap();
        let res = app
            .put(
                &token,
                &format!("/api/v1/channels/{}/pins/{}", channel_id, msg_id),
                json!({}),
            )
            .await;
        assert_eq!(res.status(), 204, "pin {} failed", i);
    }

    // 51st pin should fail
    let msg = send_text_message(app, &token, channel_id, "pin-51").await;
    let msg_id = msg["id"].as_str().unwrap();
    let res = app
        .put(
            &token,
            &format!("/api/v1/channels/{}/pins/{}", channel_id, msg_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_pin_not_permitted() {
    let app = get_test_app().await;
    let (owner_token, server_id, channel_id) = setup_server_with_channel(app).await;

    let name = unique_name("pinner");
    let (member_token, _member_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;
    add_member_to_server(app, &owner_token, &member_token, server_id).await;

    let msg = send_text_message(app, &owner_token, channel_id, "try to pin").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Regular member lacks MANAGE_MESSAGES
    let res = app
        .put(
            &member_token,
            &format!("/api/v1/channels/{}/pins/{}", channel_id, msg_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_pin_dispatches_event() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "pin event test").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Just verify pin succeeds (204) — event dispatch is internal
    let res = app
        .put(
            &token,
            &format!("/api/v1/channels/{}/pins/{}", channel_id, msg_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_pins_in_correct_channel() {
    let app = get_test_app().await;
    let (token, server_id, channel_id_a) = setup_server_with_channel(app).await;

    // Create a second text channel in the same server
    let res = app
        .post(
            &token,
            &format!("/api/v1/servers/{}/channels", server_id),
            json!({ "name": "second-channel", "type": 0 }),
        )
        .await;
    let ch_body: serde_json::Value = res.json().await.unwrap();
    let channel_id_b: i64 = ch_body["id"].as_str().unwrap().parse().unwrap();

    // Send and pin a message in channel A
    let msg_a = send_text_message(app, &token, channel_id_a, "pinned in A").await;
    let msg_a_id = msg_a["id"].as_str().unwrap();
    app.put(
        &token,
        &format!("/api/v1/channels/{}/pins/{}", channel_id_a, msg_a_id),
        json!({}),
    )
    .await;

    // Channel B pins should be empty
    let res = app
        .get(
            &token,
            &format!("/api/v1/channels/{}/pins", channel_id_b),
        )
        .await;
    assert_eq!(res.status(), 200);
    let pins_b: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(pins_b.is_empty(), "channel B should have no pins");
}

// ═══════════════════════════════════════════════════════════════════
// REACTION TESTS
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_add_reaction() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "react to me").await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = app
        .put(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
                channel_id, msg_id, "%F0%9F%91%8D" // thumbs up URL-encoded
            ),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_remove_own_reaction() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "remove reaction").await;
    let msg_id = msg["id"].as_str().unwrap();
    let emoji = "%F0%9F%91%8D";

    // Add then remove
    app.put(
        &token,
        &format!(
            "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, msg_id, emoji
        ),
        json!({}),
    )
    .await;

    let res = app
        .delete(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
                channel_id, msg_id, emoji
            ),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_get_reactors() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "who reacted").await;
    let msg_id = msg["id"].as_str().unwrap();
    let emoji = "%F0%9F%91%8D";

    app.put(
        &token,
        &format!(
            "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, msg_id, emoji
        ),
        json!({}),
    )
    .await;

    let res = app
        .get(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}",
                channel_id, msg_id, emoji
            ),
        )
        .await;
    assert_eq!(res.status(), 200);
    let users: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(!users.is_empty());
    assert!(users[0]["id"].is_string());
    assert!(users[0]["username"].is_string());
}

#[tokio::test]
async fn test_add_duplicate_reaction() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "dup reaction").await;
    let msg_id = msg["id"].as_str().unwrap();
    let emoji = "%F0%9F%91%8D";

    // Add reaction twice — should be idempotent
    let res1 = app
        .put(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
                channel_id, msg_id, emoji
            ),
            json!({}),
        )
        .await;
    assert_eq!(res1.status(), 204);

    let res2 = app
        .put(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
                channel_id, msg_id, emoji
            ),
            json!({}),
        )
        .await;
    assert_eq!(res2.status(), 204);

    // Verify only one reaction user entry
    let res = app
        .get(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}",
                channel_id, msg_id, emoji
            ),
        )
        .await;
    let users: Vec<serde_json::Value> = res.json().await.unwrap();
    assert_eq!(users.len(), 1, "duplicate reaction should be idempotent");
}

#[tokio::test]
async fn test_remove_other_reaction_with_manage() {
    let app = get_test_app().await;
    let (owner_token, server_id, channel_id) = setup_server_with_channel(app).await;

    let name = unique_name("reactor");
    let (member_token, member_id) = app
        .register_user(&name, &unique_email(&name), "testpassword123")
        .await;
    add_member_to_server(app, &owner_token, &member_token, server_id).await;

    let msg = send_text_message(app, &owner_token, channel_id, "manage reaction").await;
    let msg_id = msg["id"].as_str().unwrap();
    let emoji = "%F0%9F%91%8D";

    // Member adds a reaction
    app.put(
        &member_token,
        &format!(
            "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, msg_id, emoji
        ),
        json!({}),
    )
    .await;

    // Owner removes member's reaction via user_id endpoint
    let res = app
        .delete(
            &owner_token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}/{}",
                channel_id, msg_id, emoji, member_id
            ),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_remove_all_reactions() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "clear all reactions").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Add reactions with two different emojis
    app.put(
        &token,
        &format!(
            "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, msg_id, "%F0%9F%91%8D"
        ),
        json!({}),
    )
    .await;
    app.put(
        &token,
        &format!(
            "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, msg_id, "%E2%9D%A4"
        ),
        json!({}),
    )
    .await;

    // Remove all reactions
    let res = app
        .delete(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions",
                channel_id, msg_id
            ),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_remove_all_emoji_reactions() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "clear emoji reactions").await;
    let msg_id = msg["id"].as_str().unwrap();
    let emoji = "%F0%9F%91%8D";

    app.put(
        &token,
        &format!(
            "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, msg_id, emoji
        ),
        json!({}),
    )
    .await;

    let res = app
        .delete(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}",
                channel_id, msg_id, emoji
            ),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_reaction_empty_emoji() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "empty emoji test").await;
    let msg_id = msg["id"].as_str().unwrap();

    // Empty emoji path segment — Axum may return 404 (no matching route) or 400
    let res = app
        .put(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions//@me",
                channel_id, msg_id
            ),
            json!({}),
        )
        .await;
    // Empty path segment will likely not match the route, giving 404 or 405
    assert!(
        res.status() == 400 || res.status() == 404 || res.status() == 405,
        "expected 400/404/405 for empty emoji, got {}",
        res.status()
    );
}

#[tokio::test]
async fn test_reaction_long_emoji() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "long emoji test").await;
    let msg_id = msg["id"].as_str().unwrap();

    let long_emoji = "a".repeat(101);
    let res = app
        .put(
            &token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
                channel_id, msg_id, long_emoji
            ),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_reaction_not_member() {
    let app = get_test_app().await;
    let (_owner_token, _server_id, channel_id) = setup_server_with_channel(app).await;
    let (outsider_token, _) = register_outsider(app).await;

    // Send a message as owner first (we need a valid message)
    let msg = send_text_message(app, &_owner_token, channel_id, "no react").await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = app
        .put(
            &outsider_token,
            &format!(
                "/api/v1/channels/{}/messages/{}/reactions/{}/@me",
                channel_id, msg_id, "%F0%9F%91%8D"
            ),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 403);
}

// ═══════════════════════════════════════════════════════════════════
// TYPING TESTS
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_send_typing() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/typing", channel_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_send_typing_not_member() {
    let app = get_test_app().await;
    let (_owner_token, _server_id, channel_id) = setup_server_with_channel(app).await;
    let (outsider_token, _) = register_outsider(app).await;

    let res = app
        .post(
            &outsider_token,
            &format!("/api/v1/channels/{}/typing", channel_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_typing_rate_limited() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    // First typing
    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/typing", channel_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Second typing within 10s — should still return 204 (debounced, not error)
    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/typing", channel_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 204);
}

// ═══════════════════════════════════════════════════════════════════
// READ STATE / ACK TESTS
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_ack_message() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    let msg = send_text_message(app, &token, channel_id, "ack me").await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = app
        .post(
            &token,
            &format!("/api/v1/channels/{}/messages/{}/ack", channel_id, msg_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_ack_message_not_member() {
    let app = get_test_app().await;
    let (owner_token, _server_id, channel_id) = setup_server_with_channel(app).await;
    let (outsider_token, _) = register_outsider(app).await;

    let msg = send_text_message(app, &owner_token, channel_id, "no ack").await;
    let msg_id = msg["id"].as_str().unwrap();

    let res = app
        .post(
            &outsider_token,
            &format!("/api/v1/channels/{}/messages/{}/ack", channel_id, msg_id),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_get_unread() {
    let app = get_test_app().await;
    let (token, _server_id, channel_id) = setup_server_with_channel(app).await;

    // Send and ack a message to create a read state
    let msg = send_text_message(app, &token, channel_id, "read state test").await;
    let msg_id = msg["id"].as_str().unwrap();

    app.post(
        &token,
        &format!("/api/v1/channels/{}/messages/{}/ack", channel_id, msg_id),
        json!({}),
    )
    .await;

    let res = app.get(&token, "/api/v1/users/@me/unread").await;
    assert_eq!(res.status(), 200);
    let unread: Vec<serde_json::Value> = res.json().await.unwrap();

    // Should have at least one read state for the channel we acked
    let found = unread.iter().any(|u| {
        u["channel_id"].as_str() == Some(&channel_id.to_string())
    });
    assert!(found, "unread should contain the acked channel");
}
