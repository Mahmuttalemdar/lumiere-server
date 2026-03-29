mod common;
use common::{get_test_app, unique_name, unique_email};

// ═══════════════════════════════════════════════════════════════════
//  Helper functions
// ═══════════════════════════════════════════════════════════════════

/// Register a fresh user and create a server, returning (token, user_id, server_id).
async fn setup_server(prefix: &str) -> (String, i64, i64) {
    let app = get_test_app().await;
    let name = unique_name(prefix);
    let email = unique_email(prefix);
    let (token, user_id) = app.register_user(&name, &email, "TestPass123!").await;
    let server_id = app.create_server(&token, &unique_name("srv")).await;
    (token, user_id, server_id)
}

/// Register a second user and join them to a server via invite.
/// Returns (token, user_id).
async fn join_server(server_id: i64, owner_token: &str) -> (String, i64) {
    let app = get_test_app().await;

    // Get the first text channel to create an invite on
    let channels_res = app.get(owner_token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = channels_res.json().await.unwrap();
    let channel_id = channels.iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .expect("no text channel found")["id"]
        .as_str().unwrap()
        .parse::<i64>().unwrap();

    // Create invite
    let invite_res = app.post(
        owner_token,
        &format!("/api/v1/channels/{}/invites", channel_id),
        serde_json::json!({ "max_age": 0 }),
    ).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap().to_string();

    // Register new user
    let name = unique_name("joiner");
    let email = unique_email("joiner");
    let (token, user_id) = app.register_user(&name, &email, "TestPass123!").await;

    // Use invite
    let _join = app.post(&token, &format!("/api/v1/invites/{}", code), serde_json::json!({})).await;

    (token, user_id)
}

/// Get the first text channel id for a server.
async fn get_text_channel_id(token: &str, server_id: i64) -> i64 {
    let app = get_test_app().await;
    let res = app.get(token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = res.json().await.unwrap();
    channels.iter()
        .find(|c| c["type"].as_i64() == Some(0))
        .expect("no text channel")["id"]
        .as_str().unwrap()
        .parse::<i64>().unwrap()
}

/// Get the @everyone role id for a server (same as server_id).
fn everyone_role_id(server_id: i64) -> i64 {
    server_id
}

// ═══════════════════════════════════════════════════════════════════
//  Server CRUD Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_server() {
    let app = get_test_app().await;
    let (token, _user_id) = app.register_user(
        &unique_name("cs"), &unique_email("cs"), "TestPass123!",
    ).await;

    let res = app.post(&token, "/api/v1/servers", serde_json::json!({
        "name": "My Test Server"
    })).await;
    assert_eq!(res.status(), 201);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "My Test Server");
    assert!(body["id"].as_str().is_some());
    assert!(body["owner_id"].as_str().is_some());
    assert!(body["created_at"].as_str().is_some());
}

#[tokio::test]
async fn test_create_server_member_count() {
    let app = get_test_app().await;
    let (token, _uid) = app.register_user(
        &unique_name("mc"), &unique_email("mc"), "TestPass123!",
    ).await;

    let res = app.post(&token, "/api/v1/servers", serde_json::json!({
        "name": "Count Server"
    })).await;
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["member_count"].as_i64().unwrap(), 1);
}

#[tokio::test]
async fn test_create_server_has_general_channel() {
    let (token, _uid, server_id) = setup_server("gc").await;
    let app = get_test_app().await;

    let res = app.get(&token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    assert_eq!(res.status(), 200);

    let channels: Vec<serde_json::Value> = res.json().await.unwrap();
    let general = channels.iter().find(|c| {
        c["type"].as_i64() == Some(0) && c["name"].as_str() == Some("general")
    });
    assert!(general.is_some(), "Server should have a default 'general' text channel");
}

#[tokio::test]
async fn test_create_server_has_voice_channel() {
    let (token, _uid, server_id) = setup_server("vc").await;
    let app = get_test_app().await;

    let res = app.get(&token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = res.json().await.unwrap();
    let voice = channels.iter().find(|c| c["type"].as_i64() == Some(2));
    assert!(voice.is_some(), "Server should have a default voice channel");
}

#[tokio::test]
async fn test_get_server() {
    let (token, _uid, server_id) = setup_server("gs").await;
    let app = get_test_app().await;

    let res = app.get(&token, &format!("/api/v1/servers/{}", server_id)).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap().parse::<i64>().unwrap(), server_id);
    assert!(body["name"].as_str().is_some());
    assert!(body["owner_id"].as_str().is_some());
}

#[tokio::test]
async fn test_get_server_not_member() {
    let (_token, _uid, server_id) = setup_server("gsnm").await;
    let app = get_test_app().await;

    // Register a second user who is NOT a member
    let (other_token, _) = app.register_user(
        &unique_name("outsider"), &unique_email("outsider"), "TestPass123!",
    ).await;

    let res = app.get(&other_token, &format!("/api/v1/servers/{}", server_id)).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_update_server_name() {
    let (token, _uid, server_id) = setup_server("usn").await;
    let app = get_test_app().await;

    let res = app.patch(&token, &format!("/api/v1/servers/{}", server_id), serde_json::json!({
        "name": "Updated Name"
    })).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "Updated Name");
}

#[tokio::test]
async fn test_update_server_not_permitted() {
    let (owner_token, _uid, server_id) = setup_server("usnp").await;

    // Join a second user
    let (member_token, _member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    // Non-admin member tries to update
    let res = app.patch(&member_token, &format!("/api/v1/servers/{}", server_id), serde_json::json!({
        "name": "Hijacked"
    })).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_delete_server() {
    let (token, _uid, server_id) = setup_server("ds").await;
    let app = get_test_app().await;

    let res = app.delete_with_body(&token, &format!("/api/v1/servers/{}", server_id), serde_json::json!({
        "password": "TestPass123!"
    })).await;
    assert_eq!(res.status(), 204);

    // Verify server is gone
    let res2 = app.get(&token, &format!("/api/v1/servers/{}", server_id)).await;
    assert!(res2.status() == 403 || res2.status() == 404);
}

#[tokio::test]
async fn test_delete_server_wrong_password() {
    let (token, _uid, server_id) = setup_server("dswp").await;
    let app = get_test_app().await;

    let res = app.delete_with_body(&token, &format!("/api/v1/servers/{}", server_id), serde_json::json!({
        "password": "WrongPassword123!"
    })).await;
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_delete_server_not_owner() {
    let (owner_token, _uid, server_id) = setup_server("dsno").await;
    let (member_token, _member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.delete_with_body(&member_token, &format!("/api/v1/servers/{}", server_id), serde_json::json!({
        "password": "TestPass123!"
    })).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_server_limit_100() {
    let app = get_test_app().await;
    let (token, user_id) = app.register_user(
        &unique_name("sl"), &unique_email("sl"), "TestPass123!",
    ).await;

    // Use direct SQL to insert 100 servers as members (to hit the limit fast)
    for i in 0..100 {
        let sid = app.state.snowflake.next_id();
        sqlx::query("INSERT INTO servers (id, name, owner_id, member_count) VALUES ($1, $2, $3, 1)")
            .bind(sid)
            .bind(format!("limit_srv_{}", i))
            .bind(user_id)
            .execute(&app.state.db.pg)
            .await
            .unwrap();
        sqlx::query("INSERT INTO server_members (server_id, user_id) VALUES ($1, $2)")
            .bind(sid)
            .bind(user_id)
            .execute(&app.state.db.pg)
            .await
            .unwrap();
    }

    // 101st server should fail
    let res = app.post(&token, "/api/v1/servers", serde_json::json!({
        "name": "Over The Limit"
    })).await;
    assert_eq!(res.status(), 400);
}

// ═══════════════════════════════════════════════════════════════════
//  Invite Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_invite() {
    let (token, _uid, server_id) = setup_server("ci").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    let res = app.post(&token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 3600
    })).await;
    assert_eq!(res.status(), 201);

    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["code"].as_str().is_some());
    assert_eq!(body["uses"].as_i64().unwrap(), 0);
}

#[tokio::test]
async fn test_get_invite_preview() {
    let (token, _uid, server_id) = setup_server("gip").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    let invite_res = app.post(&token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    // No auth required for preview
    let res = app.get_unauth(&format!("/api/v1/invites/{}", code)).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["server"]["name"].as_str().is_some());
    assert!(body["server"]["member_count"].as_i64().is_some());
}

#[tokio::test]
async fn test_use_invite() {
    let (owner_token, _uid, server_id) = setup_server("ui").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&owner_token, server_id).await;

    let invite_res = app.post(&owner_token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    let (new_token, _new_uid) = app.register_user(
        &unique_name("invitee"), &unique_email("invitee"), "TestPass123!",
    ).await;

    let res = app.post(&new_token, &format!("/api/v1/invites/{}", code), serde_json::json!({})).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["member_count"].as_i64().unwrap(), 2);
}

#[tokio::test]
async fn test_use_invite_already_member() {
    let (owner_token, _uid, server_id) = setup_server("uiam").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&owner_token, server_id).await;

    let invite_res = app.post(&owner_token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    // Owner is already a member — use invite again
    let res = app.post(&owner_token, &format!("/api/v1/invites/{}", code), serde_json::json!({})).await;
    assert_eq!(res.status(), 200);

    // Should return server without error
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["id"].as_str().is_some());
}

#[tokio::test]
async fn test_use_invite_banned() {
    let (owner_token, _uid, server_id) = setup_server("uib").await;
    let (member_token, member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    // Ban the member
    app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, member_id), serde_json::json!({
        "reason": "testing"
    })).await;

    // Create another invite
    let channel_id = get_text_channel_id(&owner_token, server_id).await;
    let invite_res = app.post(&owner_token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    // Banned user tries to join
    let res = app.post(&member_token, &format!("/api/v1/invites/{}", code), serde_json::json!({})).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_use_invite_max_uses() {
    let (owner_token, _uid, server_id) = setup_server("uimu").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&owner_token, server_id).await;

    // Create invite with max_uses = 1
    let invite_res = app.post(&owner_token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0,
        "max_uses": 1
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap().to_string();

    // First user joins
    let (t1, _) = app.register_user(
        &unique_name("mu1"), &unique_email("mu1"), "TestPass123!",
    ).await;
    let res1 = app.post(&t1, &format!("/api/v1/invites/{}", code), serde_json::json!({})).await;
    assert_eq!(res1.status(), 200);

    // Second user — should fail (invite preview returns expired/max-uses)
    let (t2, _) = app.register_user(
        &unique_name("mu2"), &unique_email("mu2"), "TestPass123!",
    ).await;
    let res2 = app.post(&t2, &format!("/api/v1/invites/{}", code), serde_json::json!({})).await;
    assert_eq!(res2.status(), 400);
}

#[tokio::test]
async fn test_use_invite_expired() {
    let (owner_token, _uid, server_id) = setup_server("uie").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&owner_token, server_id).await;

    // Create invite with max_age = 1 second
    let invite_res = app.post(&owner_token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 1
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    // Wait for expiry
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let (t, _) = app.register_user(
        &unique_name("exp"), &unique_email("exp"), "TestPass123!",
    ).await;
    let res = app.post(&t, &format!("/api/v1/invites/{}", code), serde_json::json!({})).await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_list_server_invites() {
    let (token, _uid, server_id) = setup_server("lsi").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    // Create two invites
    app.post(&token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    app.post(&token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;

    let res = app.get(&token, &format!("/api/v1/servers/{}/invites", server_id)).await;
    assert_eq!(res.status(), 200);

    let body: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(body.len() >= 2);
}

#[tokio::test]
async fn test_delete_invite() {
    let (token, _uid, server_id) = setup_server("di").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    let invite_res = app.post(&token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    let res = app.delete(&token, &format!("/api/v1/invites/{}", code)).await;
    assert_eq!(res.status(), 204);

    // Verify invite is gone
    let res2 = app.get_unauth(&format!("/api/v1/invites/{}", code)).await;
    assert_eq!(res2.status(), 404);
}

#[tokio::test]
async fn test_delete_invite_by_creator() {
    let (owner_token, _uid, server_id) = setup_server("dibc").await;
    let (member_token, _member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    // Grant CREATE_INVITE to the member via @everyone (already default)
    let channel_id = get_text_channel_id(&owner_token, server_id).await;

    // Member creates an invite
    let invite_res = app.post(&member_token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    assert_eq!(invite_res.status(), 201);
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    // Member deletes own invite
    let res = app.delete(&member_token, &format!("/api/v1/invites/{}", code)).await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_invite_not_authorized() {
    let (owner_token, _uid, server_id) = setup_server("dina").await;
    let (member_token, _member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let channel_id = get_text_channel_id(&owner_token, server_id).await;

    // Owner creates invite
    let invite_res = app.post(&owner_token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    // Non-creator member without MANAGE_SERVER tries to delete
    let res = app.delete(&member_token, &format!("/api/v1/invites/{}", code)).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_invite_code_format() {
    let (token, _uid, server_id) = setup_server("icf").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    let invite_res = app.post(&token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    assert_eq!(code.len(), 8, "Invite code should be 8 characters");
}

// ═══════════════════════════════════════════════════════════════════
//  Member Management Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_list_members() {
    let (owner_token, _uid, server_id) = setup_server("lm").await;
    let (_member_token, _member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.get(&owner_token, &format!("/api/v1/servers/{}/members", server_id)).await;
    assert_eq!(res.status(), 200);

    let body: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(body.len() >= 2);
    // Each member should have user and roles
    assert!(body[0]["user"]["id"].as_str().is_some());
    assert!(body[0]["roles"].as_array().is_some());
}

#[tokio::test]
async fn test_list_members_pagination() {
    let (owner_token, _uid, server_id) = setup_server("lmp").await;
    // Add a couple members
    let (_t1, _id1) = join_server(server_id, &owner_token).await;
    let (_t2, _id2) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    // Page with limit=1
    let res = app.get(&owner_token, &format!("/api/v1/servers/{}/members?limit=1", server_id)).await;
    let page1: Vec<serde_json::Value> = res.json().await.unwrap();
    assert_eq!(page1.len(), 1);

    // Use `after` for cursor
    let first_id = page1[0]["user"]["id"].as_str().unwrap();
    let res2 = app.get(&owner_token, &format!("/api/v1/servers/{}/members?limit=1&after={}", server_id, first_id)).await;
    let page2: Vec<serde_json::Value> = res2.json().await.unwrap();
    assert_eq!(page2.len(), 1);
    assert_ne!(page2[0]["user"]["id"].as_str().unwrap(), first_id);
}

#[tokio::test]
async fn test_get_member() {
    let (owner_token, _uid, server_id) = setup_server("gm").await;
    let (_member_token, member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.get(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, member_id)).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["user"]["id"].as_str().unwrap().parse::<i64>().unwrap(), member_id);
}

#[tokio::test]
async fn test_update_member_nickname() {
    let (owner_token, _uid, server_id) = setup_server("umn").await;
    let (_member_token, member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.patch(
        &owner_token,
        &format!("/api/v1/servers/{}/members/{}", server_id, member_id),
        serde_json::json!({ "nickname": "Cool Nick" }),
    ).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["nickname"].as_str().unwrap(), "Cool Nick");
}

#[tokio::test]
async fn test_update_self_nickname() {
    let (owner_token, _uid, server_id) = setup_server("usn2").await;
    let (member_token, _member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.patch(
        &member_token,
        &format!("/api/v1/servers/{}/members/@me", server_id),
        serde_json::json!({ "nickname": "My Nick" }),
    ).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["nickname"].as_str().unwrap(), "My Nick");
}

#[tokio::test]
async fn test_kick_member() {
    let (owner_token, _uid, server_id) = setup_server("km").await;
    let (_member_token, member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.delete(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, member_id)).await;
    assert_eq!(res.status(), 204);

    // Verify member count decreased
    let server = app.get(&owner_token, &format!("/api/v1/servers/{}", server_id)).await;
    let body: serde_json::Value = server.json().await.unwrap();
    assert_eq!(body["member_count"].as_i64().unwrap(), 1);
}

#[tokio::test]
async fn test_kick_member_not_permitted() {
    let (owner_token, _uid, server_id) = setup_server("kmnp").await;
    let (member_token, _member_id) = join_server(server_id, &owner_token).await;
    let (_target_token, target_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    // Regular member tries to kick
    let res = app.delete(&member_token, &format!("/api/v1/servers/{}/members/{}", server_id, target_id)).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_kick_owner() {
    let (owner_token, owner_id, server_id) = setup_server("ko").await;
    let app = get_test_app().await;

    // Owner tries to kick themselves — but the route for kick is used on another member, not self
    // Actually: can the owner be kicked? (yes, trying that)
    // The route is DELETE /servers/{id}/members/{user_id}
    // Owner can technically try to kick themselves, but they can't via the kick endpoint
    // Let's test with a member that has kick perms trying to kick the owner
    let (_member_token, _member_id) = join_server(server_id, &owner_token).await;

    // Owner tries to "kick" themselves via the endpoint
    let res = app.delete(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, owner_id)).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_leave_server() {
    let (owner_token, _uid, server_id) = setup_server("ls").await;
    let (member_token, _member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.delete(&member_token, &format!("/api/v1/servers/{}/leave", server_id)).await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_owner_cannot_leave() {
    let (owner_token, _uid, server_id) = setup_server("ocl").await;
    let app = get_test_app().await;

    let res = app.delete(&owner_token, &format!("/api/v1/servers/{}/leave", server_id)).await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_transfer_ownership() {
    let (owner_token, _uid, server_id) = setup_server("to").await;
    let (new_owner_token, new_owner_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    // Transfer ownership
    let res = app.patch(&owner_token, &format!("/api/v1/servers/{}", server_id), serde_json::json!({
        "owner_id": new_owner_id
    })).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["owner_id"].as_str().unwrap().parse::<i64>().unwrap(), new_owner_id);

    // New owner can update
    let res2 = app.patch(&new_owner_token, &format!("/api/v1/servers/{}", server_id), serde_json::json!({
        "name": "New Owner Server"
    })).await;
    assert_eq!(res2.status(), 200);
}

#[tokio::test]
async fn test_role_hierarchy_kick() {
    let (owner_token, _uid, server_id) = setup_server("rhk").await;
    let app = get_test_app().await;

    // Create roles — position increases with each role created.
    // "Low Role" is created first (lower position), "High Role" second (higher position).
    let low_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Low Role",
        "permissions": "32"  // KICK_MEMBERS
    })).await;
    let low_role: serde_json::Value = low_res.json().await.unwrap();
    let low_role_id = low_role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let high_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "High Role",
        "permissions": "32"  // KICK_MEMBERS
    })).await;
    let high_role: serde_json::Value = high_res.json().await.unwrap();
    let high_role_id = high_role["id"].as_str().unwrap().parse::<i64>().unwrap();

    // Add two members
    let (_high_token, high_uid) = join_server(server_id, &owner_token).await;
    let (low_token, low_uid) = join_server(server_id, &owner_token).await;

    // Assign high role to first member, low role to second
    app.patch(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, high_uid), serde_json::json!({
        "roles": [high_role_id, everyone_role_id(server_id)]
    })).await;
    app.patch(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, low_uid), serde_json::json!({
        "roles": [low_role_id, everyone_role_id(server_id)]
    })).await;

    // Low role member tries to kick high role member
    let res = app.delete(&low_token, &format!("/api/v1/servers/{}/members/{}", server_id, high_uid)).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_role_hierarchy_ban() {
    let (owner_token, _uid, server_id) = setup_server("rhb").await;
    let app = get_test_app().await;

    // Create role with BAN_MEMBERS
    let role_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Mod Role",
        "permissions": "64"  // BAN_MEMBERS
    })).await;
    let role: serde_json::Value = role_res.json().await.unwrap();
    let mod_role_id = role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let high_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "High Role"
    })).await;
    let high_role: serde_json::Value = high_res.json().await.unwrap();
    let high_role_id = high_role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let (mod_token, mod_uid) = join_server(server_id, &owner_token).await;
    let (_target_token, target_uid) = join_server(server_id, &owner_token).await;

    // Give mod the lower-positioned role, target the higher-positioned role
    app.patch(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, mod_uid), serde_json::json!({
        "roles": [mod_role_id, everyone_role_id(server_id)]
    })).await;
    app.patch(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, target_uid), serde_json::json!({
        "roles": [high_role_id, everyone_role_id(server_id)]
    })).await;

    // Mod (lower role) tries to ban target (higher role)
    let res = app.put(&mod_token, &format!("/api/v1/servers/{}/bans/{}", server_id, target_uid), serde_json::json!({
        "reason": "test"
    })).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_role_hierarchy_update() {
    let (owner_token, _uid, server_id) = setup_server("rhu").await;
    let app = get_test_app().await;

    // Create roles with MANAGE_NICKNAMES
    let low_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Low Mod",
        "permissions": "512"  // MANAGE_NICKNAMES
    })).await;
    let low_role: serde_json::Value = low_res.json().await.unwrap();
    let low_role_id = low_role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let high_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "High Member"
    })).await;
    let high_role: serde_json::Value = high_res.json().await.unwrap();
    let high_role_id = high_role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let (low_token, low_uid) = join_server(server_id, &owner_token).await;
    let (_high_token, high_uid) = join_server(server_id, &owner_token).await;

    app.patch(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, low_uid), serde_json::json!({
        "roles": [low_role_id, everyone_role_id(server_id)]
    })).await;
    app.patch(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, high_uid), serde_json::json!({
        "roles": [high_role_id, everyone_role_id(server_id)]
    })).await;

    // Low mod tries to change nickname of higher role member
    let res = app.patch(&low_token, &format!("/api/v1/servers/{}/members/{}", server_id, high_uid), serde_json::json!({
        "nickname": "No Way"
    })).await;
    assert_eq!(res.status(), 403);
}

// ═══════════════════════════════════════════════════════════════════
//  Ban Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_ban() {
    let (owner_token, _uid, server_id) = setup_server("cb").await;
    let (_member_token, member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, member_id), serde_json::json!({
        "reason": "Bad behavior"
    })).await;
    assert_eq!(res.status(), 204);

    // Verify member is removed
    let members_res = app.get(&owner_token, &format!("/api/v1/servers/{}/members", server_id)).await;
    let members: Vec<serde_json::Value> = members_res.json().await.unwrap();
    let banned_member = members.iter().find(|m| {
        m["user"]["id"].as_str().unwrap().parse::<i64>().unwrap() == member_id
    });
    assert!(banned_member.is_none(), "Banned member should be removed from member list");
}

#[tokio::test]
async fn test_create_ban_not_permitted() {
    let (owner_token, _uid, server_id) = setup_server("cbnp").await;
    let (member_token, _member_id) = join_server(server_id, &owner_token).await;
    let (_target_token, target_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    let res = app.put(&member_token, &format!("/api/v1/servers/{}/bans/{}", server_id, target_id), serde_json::json!({
        "reason": "Nope"
    })).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_list_bans() {
    let (owner_token, _uid, server_id) = setup_server("lb").await;
    let (_t1, uid1) = join_server(server_id, &owner_token).await;
    let (_t2, uid2) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, uid1), serde_json::json!({
        "reason": "ban1"
    })).await;
    app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, uid2), serde_json::json!({
        "reason": "ban2"
    })).await;

    let res = app.get(&owner_token, &format!("/api/v1/servers/{}/bans", server_id)).await;
    assert_eq!(res.status(), 200);

    let bans: Vec<serde_json::Value> = res.json().await.unwrap();
    assert_eq!(bans.len(), 2);
}

#[tokio::test]
async fn test_get_ban() {
    let (owner_token, _uid, server_id) = setup_server("gb").await;
    let (_t, uid) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, uid), serde_json::json!({
        "reason": "naughty"
    })).await;

    let res = app.get(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, uid)).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["reason"].as_str().unwrap(), "naughty");
    assert!(body["user"]["id"].as_str().is_some());
}

#[tokio::test]
async fn test_remove_ban() {
    let (owner_token, _uid, server_id) = setup_server("rb").await;
    let (_t, uid) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, uid), serde_json::json!({
        "reason": "temp"
    })).await;

    let res = app.delete(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, uid)).await;
    assert_eq!(res.status(), 204);

    // Verify ban is gone
    let res2 = app.get(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, uid)).await;
    assert_eq!(res2.status(), 404);
}

#[tokio::test]
async fn test_ban_owner() {
    let (owner_token, owner_id, server_id) = setup_server("bo").await;
    let app = get_test_app().await;

    let res = app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, owner_id), serde_json::json!({
        "reason": "self-ban"
    })).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_banned_user_cannot_rejoin() {
    let (owner_token, _uid, server_id) = setup_server("bucr").await;
    let (member_token, member_id) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    // Ban the member
    app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, member_id), serde_json::json!({
        "reason": "gone"
    })).await;

    // Create new invite
    let channel_id = get_text_channel_id(&owner_token, server_id).await;
    let invite_res = app.post(&owner_token, &format!("/api/v1/channels/{}/invites", channel_id), serde_json::json!({
        "max_age": 0
    })).await;
    let invite: serde_json::Value = invite_res.json().await.unwrap();
    let code = invite["code"].as_str().unwrap();

    let res = app.post(&member_token, &format!("/api/v1/invites/{}", code), serde_json::json!({})).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_ban_removes_membership() {
    let (owner_token, _uid, server_id) = setup_server("brm").await;
    let (_t, uid) = join_server(server_id, &owner_token).await;
    let app = get_test_app().await;

    // Confirm member exists
    let before = app.get(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, uid)).await;
    assert_eq!(before.status(), 200);

    // Ban
    app.put(&owner_token, &format!("/api/v1/servers/{}/bans/{}", server_id, uid), serde_json::json!({
        "reason": "test"
    })).await;

    // Confirm member is gone
    let after = app.get(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, uid)).await;
    assert_eq!(after.status(), 404);
}

// ═══════════════════════════════════════════════════════════════════
//  Channel Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_text_channel() {
    let (token, _uid, server_id) = setup_server("ctc").await;
    let app = get_test_app().await;

    let res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "new-text",
        "type": 0
    })).await;
    assert_eq!(res.status(), 201);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["type"].as_i64().unwrap(), 0);
    assert_eq!(body["name"].as_str().unwrap(), "new-text");
}

#[tokio::test]
async fn test_create_voice_channel() {
    let (token, _uid, server_id) = setup_server("cvc").await;
    let app = get_test_app().await;

    let res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "Voice Room",
        "type": 2,
        "bitrate": 64000
    })).await;
    assert_eq!(res.status(), 201);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["type"].as_i64().unwrap(), 2);
    assert_eq!(body["bitrate"].as_i64().unwrap(), 64000);
}

#[tokio::test]
async fn test_create_category() {
    let (token, _uid, server_id) = setup_server("cc").await;
    let app = get_test_app().await;

    let res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "My Category",
        "type": 4
    })).await;
    assert_eq!(res.status(), 201);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["type"].as_i64().unwrap(), 4);
}

#[tokio::test]
async fn test_create_channel_in_category() {
    let (token, _uid, server_id) = setup_server("ccic").await;
    let app = get_test_app().await;

    // Create category
    let cat_res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "Cat",
        "type": 4
    })).await;
    let cat: serde_json::Value = cat_res.json().await.unwrap();
    let cat_id = cat["id"].as_str().unwrap().parse::<i64>().unwrap();

    // Create channel in category
    let res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "sub-channel",
        "type": 0,
        "parent_id": cat_id
    })).await;
    assert_eq!(res.status(), 201);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["parent_id"].as_str().unwrap().parse::<i64>().unwrap(), cat_id);
}

#[tokio::test]
async fn test_create_channel_max_500() {
    let (token, _uid, server_id) = setup_server("cm500").await;
    let app = get_test_app().await;

    // Insert 498 channels via SQL (already have 2 default channels)
    for i in 0..498 {
        let cid = app.state.snowflake.next_id();
        sqlx::query("INSERT INTO channels (id, server_id, type, name, position) VALUES ($1, $2, 0, $3, $4)")
            .bind(cid)
            .bind(server_id)
            .bind(format!("ch-{}", i))
            .bind(i + 2)
            .execute(&app.state.db.pg)
            .await
            .unwrap();
    }

    // 501st channel should fail
    let res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "over-limit",
        "type": 0
    })).await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_create_channel_max_50_per_category() {
    let (token, _uid, server_id) = setup_server("cm50c").await;
    let app = get_test_app().await;

    // Create category
    let cat_res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "Full Category",
        "type": 4
    })).await;
    let cat: serde_json::Value = cat_res.json().await.unwrap();
    let cat_id = cat["id"].as_str().unwrap().parse::<i64>().unwrap();

    // Insert 50 channels in category via SQL
    for i in 0..50 {
        let cid = app.state.snowflake.next_id();
        sqlx::query("INSERT INTO channels (id, server_id, parent_id, type, name, position) VALUES ($1, $2, $3, 0, $4, $5)")
            .bind(cid)
            .bind(server_id)
            .bind(cat_id)
            .bind(format!("cat-ch-{}", i))
            .bind(i)
            .execute(&app.state.db.pg)
            .await
            .unwrap();
    }

    // 51st should fail
    let res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "overflow",
        "type": 0,
        "parent_id": cat_id
    })).await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_get_channel() {
    let (token, _uid, server_id) = setup_server("gch").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    let res = app.get(&token, &format!("/api/v1/channels/{}", channel_id)).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap().parse::<i64>().unwrap(), channel_id);
    assert!(body["permission_overrides"].as_array().is_some());
}

#[tokio::test]
async fn test_update_channel_name() {
    let (token, _uid, server_id) = setup_server("ucn").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    let res = app.patch(&token, &format!("/api/v1/channels/{}", channel_id), serde_json::json!({
        "name": "New Channel Name"
    })).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    // Text channels: name is normalized (lowercase, hyphens)
    assert_eq!(body["name"].as_str().unwrap(), "new-channel-name");
}

#[tokio::test]
async fn test_update_channel_topic() {
    let (token, _uid, server_id) = setup_server("uct").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    let res = app.patch(&token, &format!("/api/v1/channels/{}", channel_id), serde_json::json!({
        "topic": "This is the topic"
    })).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["topic"].as_str().unwrap(), "This is the topic");
}

#[tokio::test]
async fn test_delete_channel() {
    let (token, _uid, server_id) = setup_server("dc").await;
    let app = get_test_app().await;

    // Create a second text channel first (can't delete last one)
    let new_ch = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "deletable",
        "type": 0
    })).await;
    let ch: serde_json::Value = new_ch.json().await.unwrap();
    let ch_id = ch["id"].as_str().unwrap().parse::<i64>().unwrap();

    let res = app.delete(&token, &format!("/api/v1/channels/{}", ch_id)).await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_last_text_channel() {
    let (token, _uid, server_id) = setup_server("dltc").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    let res = app.delete(&token, &format!("/api/v1/channels/{}", channel_id)).await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_delete_category_moves_children() {
    let (token, _uid, server_id) = setup_server("dcmc").await;
    let app = get_test_app().await;

    // Create category
    let cat_res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "Del Category",
        "type": 4
    })).await;
    let cat: serde_json::Value = cat_res.json().await.unwrap();
    let cat_id = cat["id"].as_str().unwrap().parse::<i64>().unwrap();

    // Create child channel
    let child_res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "child-ch",
        "type": 0,
        "parent_id": cat_id
    })).await;
    let child: serde_json::Value = child_res.json().await.unwrap();
    let child_id = child["id"].as_str().unwrap().parse::<i64>().unwrap();

    // Delete category
    let del_res = app.delete(&token, &format!("/api/v1/channels/{}", cat_id)).await;
    assert_eq!(del_res.status(), 204);

    // Child should now have no parent
    let child_after = app.get(&token, &format!("/api/v1/channels/{}", child_id)).await;
    let body: serde_json::Value = child_after.json().await.unwrap();
    assert!(body["parent_id"].is_null(), "Child should become parentless after category deletion");
}

#[tokio::test]
async fn test_reorder_channels() {
    let (token, _uid, server_id) = setup_server("rc").await;
    let app = get_test_app().await;

    // Create second channel
    let ch_res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "second-channel",
        "type": 0
    })).await;
    let ch: serde_json::Value = ch_res.json().await.unwrap();
    let ch_id = ch["id"].as_str().unwrap().parse::<i64>().unwrap();

    let original_id = get_text_channel_id(&token, server_id).await;

    let res = app.patch(&token, &format!("/api/v1/servers/{}/channels/reorder", server_id), serde_json::json!([
        { "id": original_id, "position": 1 },
        { "id": ch_id, "position": 0 }
    ])).await;
    assert_eq!(res.status(), 204);

    // Verify order changed
    let channels_res = app.get(&token, &format!("/api/v1/servers/{}/channels", server_id)).await;
    let channels: Vec<serde_json::Value> = channels_res.json().await.unwrap();
    let first_text = channels.iter().find(|c| c["type"].as_i64() == Some(0) && c["position"].as_i64() == Some(0));
    assert!(first_text.is_some());
    assert_eq!(first_text.unwrap()["id"].as_str().unwrap().parse::<i64>().unwrap(), ch_id);
}

#[tokio::test]
async fn test_channel_name_normalization() {
    let (token, _uid, server_id) = setup_server("cnn").await;
    let app = get_test_app().await;

    let res = app.post(&token, &format!("/api/v1/servers/{}/channels", server_id), serde_json::json!({
        "name": "  Hello World 123  ",
        "type": 0
    })).await;
    assert_eq!(res.status(), 201);

    let body: serde_json::Value = res.json().await.unwrap();
    // Text channel names: lowercase, spaces->hyphens, trimmed
    assert_eq!(body["name"].as_str().unwrap(), "hello-world-123");
}

// ═══════════════════════════════════════════════════════════════════
//  Permission / Role Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_role() {
    let (token, _uid, server_id) = setup_server("cr").await;
    let app = get_test_app().await;

    let res = app.post(&token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Moderator"
    })).await;
    assert_eq!(res.status(), 201);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "Moderator");
    assert!(body["position"].as_i64().unwrap() > 0);
    assert_eq!(body["is_default"].as_bool().unwrap(), false);
}

#[tokio::test]
async fn test_list_roles() {
    let (token, _uid, server_id) = setup_server("lr").await;
    let app = get_test_app().await;

    let res = app.get(&token, &format!("/api/v1/servers/{}/roles", server_id)).await;
    assert_eq!(res.status(), 200);

    let roles: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(!roles.is_empty());

    // Should include @everyone
    let everyone = roles.iter().find(|r| r["is_default"].as_bool() == Some(true));
    assert!(everyone.is_some(), "Should include @everyone role");
}

#[tokio::test]
async fn test_update_role_name() {
    let (token, _uid, server_id) = setup_server("urn").await;
    let app = get_test_app().await;

    let create_res = app.post(&token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "OldName"
    })).await;
    let role: serde_json::Value = create_res.json().await.unwrap();
    let role_id = role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let res = app.patch(&token, &format!("/api/v1/servers/{}/roles/{}", server_id, role_id), serde_json::json!({
        "name": "NewName"
    })).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "NewName");
}

#[tokio::test]
async fn test_update_role_permissions() {
    let (token, _uid, server_id) = setup_server("urp").await;
    let app = get_test_app().await;

    let create_res = app.post(&token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "CustomRole"
    })).await;
    let role: serde_json::Value = create_res.json().await.unwrap();
    let role_id = role["id"].as_str().unwrap().parse::<i64>().unwrap();

    // Set SEND_MESSAGES (1 << 14 = 16384)
    let res = app.patch(&token, &format!("/api/v1/servers/{}/roles/{}", server_id, role_id), serde_json::json!({
        "permissions": "16384"
    })).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["permissions"].as_str().unwrap(), "16384");
}

#[tokio::test]
async fn test_delete_role() {
    let (token, _uid, server_id) = setup_server("dr").await;
    let app = get_test_app().await;

    let create_res = app.post(&token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Deletable"
    })).await;
    let role: serde_json::Value = create_res.json().await.unwrap();
    let role_id = role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let res = app.delete(&token, &format!("/api/v1/servers/{}/roles/{}", server_id, role_id)).await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_everyone_role() {
    let (token, _uid, server_id) = setup_server("der").await;
    let app = get_test_app().await;

    // @everyone role id = server_id
    let res = app.delete(&token, &format!("/api/v1/servers/{}/roles/{}", server_id, server_id)).await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_role_hierarchy_prevents_editing_higher() {
    let (owner_token, _uid, server_id) = setup_server("rhpe").await;
    let app = get_test_app().await;

    // Create two roles
    let low_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Low",
        "permissions": "8"  // MANAGE_ROLES
    })).await;
    let low_role: serde_json::Value = low_res.json().await.unwrap();
    let low_role_id = low_role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let high_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "High"
    })).await;
    let high_role: serde_json::Value = high_res.json().await.unwrap();
    let high_role_id = high_role["id"].as_str().unwrap().parse::<i64>().unwrap();

    // Add member with low role
    let (member_token, member_id) = join_server(server_id, &owner_token).await;
    app.patch(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, member_id), serde_json::json!({
        "roles": [low_role_id, everyone_role_id(server_id)]
    })).await;

    // Member with low role tries to edit high role
    let res = app.patch(&member_token, &format!("/api/v1/servers/{}/roles/{}", server_id, high_role_id), serde_json::json!({
        "name": "Hijacked"
    })).await;
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_reorder_roles() {
    let (token, _uid, server_id) = setup_server("rr").await;
    let app = get_test_app().await;

    let r1_res = app.post(&token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Role A"
    })).await;
    let r1: serde_json::Value = r1_res.json().await.unwrap();
    let r1_id = r1["id"].as_str().unwrap().parse::<i64>().unwrap();

    let r2_res = app.post(&token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Role B"
    })).await;
    let r2: serde_json::Value = r2_res.json().await.unwrap();
    let r2_id = r2["id"].as_str().unwrap().parse::<i64>().unwrap();

    // Swap positions
    let res = app.patch(&token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!([
        { "id": r1_id, "position": 2 },
        { "id": r2_id, "position": 1 }
    ])).await;
    assert_eq!(res.status(), 200);

    let roles: Vec<serde_json::Value> = res.json().await.unwrap();
    let role_a = roles.iter().find(|r| r["id"].as_str().unwrap().parse::<i64>().unwrap() == r1_id).unwrap();
    assert_eq!(role_a["position"].as_i64().unwrap(), 2);
}

#[tokio::test]
async fn test_set_channel_override() {
    let (token, _uid, server_id) = setup_server("sco").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    // Set override for @everyone role
    let res = app.put(&token, &format!("/api/v1/channels/{}/permissions/{}", channel_id, server_id), serde_json::json!({
        "type": 0,
        "allow": "16384",
        "deny": "0"
    })).await;
    assert_eq!(res.status(), 204);

    // Verify by fetching channel
    let ch_res = app.get(&token, &format!("/api/v1/channels/{}", channel_id)).await;
    let ch: serde_json::Value = ch_res.json().await.unwrap();
    let overrides = ch["permission_overrides"].as_array().unwrap();
    assert!(!overrides.is_empty());
}

#[tokio::test]
async fn test_channel_override_strips_admin() {
    let (token, _uid, server_id) = setup_server("cosa").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    // Try to set ADMINISTRATOR bit (1 << 0 = 1)
    let res = app.put(&token, &format!("/api/v1/channels/{}/permissions/{}", channel_id, server_id), serde_json::json!({
        "type": 0,
        "allow": "1",
        "deny": "0"
    })).await;
    assert_eq!(res.status(), 204);

    // Verify ADMINISTRATOR bit is stripped
    let ch_res = app.get(&token, &format!("/api/v1/channels/{}", channel_id)).await;
    let ch: serde_json::Value = ch_res.json().await.unwrap();
    let overrides = ch["permission_overrides"].as_array().unwrap();
    if !overrides.is_empty() {
        let allow = overrides[0]["allow"].as_i64().unwrap();
        assert_eq!(allow & 1, 0, "ADMINISTRATOR bit should be stripped from channel overrides");
    }
}

#[tokio::test]
async fn test_delete_channel_override() {
    let (token, _uid, server_id) = setup_server("dco").await;
    let app = get_test_app().await;
    let channel_id = get_text_channel_id(&token, server_id).await;

    // Create override
    app.put(&token, &format!("/api/v1/channels/{}/permissions/{}", channel_id, server_id), serde_json::json!({
        "type": 0,
        "allow": "16384",
        "deny": "0"
    })).await;

    // Delete override
    let res = app.delete(&token, &format!("/api/v1/channels/{}/permissions/{}", channel_id, server_id)).await;
    assert_eq!(res.status(), 204);

    // Verify removed
    let ch_res = app.get(&token, &format!("/api/v1/channels/{}", channel_id)).await;
    let ch: serde_json::Value = ch_res.json().await.unwrap();
    let overrides = ch["permission_overrides"].as_array().unwrap();
    let found = overrides.iter().find(|o| {
        o["id"].as_str().unwrap().parse::<i64>().unwrap() == server_id
    });
    assert!(found.is_none(), "Override should be removed");
}

#[tokio::test]
async fn test_permission_escalation_prevented() {
    let (owner_token, _uid, server_id) = setup_server("pep").await;
    let app = get_test_app().await;

    // Create a limited role for a member (only MANAGE_ROLES, no ADMINISTRATOR)
    let role_res = app.post(&owner_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Limited Mod",
        "permissions": "8"  // MANAGE_ROLES only
    })).await;
    let role: serde_json::Value = role_res.json().await.unwrap();
    let role_id = role["id"].as_str().unwrap().parse::<i64>().unwrap();

    let (member_token, member_id) = join_server(server_id, &owner_token).await;
    app.patch(&owner_token, &format!("/api/v1/servers/{}/members/{}", server_id, member_id), serde_json::json!({
        "roles": [role_id, everyone_role_id(server_id)]
    })).await;

    // Member tries to create a role with ADMINISTRATOR (bit 0 = 1) — should fail
    let res = app.post(&member_token, &format!("/api/v1/servers/{}/roles", server_id), serde_json::json!({
        "name": "Escalated",
        "permissions": "1"
    })).await;
    assert_eq!(res.status(), 403);
}
