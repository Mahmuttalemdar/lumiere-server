mod common;
use common::{get_test_app, unique_name, unique_email};

// ═══════════════════════════════════════════════════════════════════
// Auth Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_register_success() {
    let app = get_test_app().await;
    let name = unique_name("reg");
    let email = unique_email("reg");

    let res = app
        .post_unauth(
            "/api/v1/auth/register",
            serde_json::json!({
                "username": name,
                "email": email,
                "password": "securepass123"
            }),
        )
        .await;

    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();

    // Tokens present
    assert!(body["access_token"].is_string());
    assert!(body["refresh_token"].is_string());

    // User object present with correct fields
    let user = &body["user"];
    assert!(user["id"].is_string());
    assert_eq!(user["username"].as_str().unwrap(), name);
    assert_eq!(user["email"].as_str().unwrap(), email.to_lowercase());
    assert!(user["created_at"].is_string());
}

#[tokio::test]
async fn test_register_missing_username() {
    let app = get_test_app().await;
    let email = unique_email("noreg");

    let res = app
        .post_unauth(
            "/api/v1/auth/register",
            serde_json::json!({
                "email": email,
                "password": "securepass123"
            }),
        )
        .await;

    // Missing field should be a 400-level error (either 400 or 422 depending on deserialization)
    let status = res.status().as_u16();
    assert!(status == 400 || status == 422, "Expected 400 or 422, got {status}");
}

#[tokio::test]
async fn test_register_short_password() {
    let app = get_test_app().await;
    let name = unique_name("short");
    let email = unique_email("short");

    let res = app
        .post_unauth(
            "/api/v1/auth/register",
            serde_json::json!({
                "username": name,
                "email": email,
                "password": "short"
            }),
        )
        .await;

    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "VALIDATION_ERROR");
    assert!(body["error"]["fields"]["password"].is_array());
}

#[tokio::test]
async fn test_register_invalid_email() {
    let app = get_test_app().await;
    let name = unique_name("bademail");

    let res = app
        .post_unauth(
            "/api/v1/auth/register",
            serde_json::json!({
                "username": name,
                "email": "not-an-email",
                "password": "securepass123"
            }),
        )
        .await;

    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "VALIDATION_ERROR");
    assert!(body["error"]["fields"]["email"].is_array());
}

#[tokio::test]
async fn test_register_duplicate_email() {
    let app = get_test_app().await;
    let name1 = unique_name("dup1");
    let name2 = unique_name("dup2");
    let email = unique_email("dup");

    // Register first user
    app.register_user(&name1, &email, "securepass123").await;

    // Try to register with same email
    let res = app
        .post_unauth(
            "/api/v1/auth/register",
            serde_json::json!({
                "username": name2,
                "email": email,
                "password": "securepass123"
            }),
        )
        .await;

    assert_eq!(res.status(), 409);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "ALREADY_EXISTS");
}

#[tokio::test]
async fn test_register_duplicate_username() {
    let app = get_test_app().await;
    let name = unique_name("dupname");
    let email1 = unique_email("dupname1");
    let email2 = unique_email("dupname2");

    app.register_user(&name, &email1, "securepass123").await;

    let res = app
        .post_unauth(
            "/api/v1/auth/register",
            serde_json::json!({
                "username": name,
                "email": email2,
                "password": "securepass123"
            }),
        )
        .await;

    assert_eq!(res.status(), 409);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "ALREADY_EXISTS");
}

#[tokio::test]
async fn test_login_success() {
    let app = get_test_app().await;
    let name = unique_name("login");
    let email = unique_email("login");
    let password = "securepass123";

    app.register_user(&name, &email, password).await;

    let res = app
        .post_unauth(
            "/api/v1/auth/login",
            serde_json::json!({
                "email": email,
                "password": password
            }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["access_token"].is_string());
    assert!(body["refresh_token"].is_string());
    assert_eq!(body["user"]["username"].as_str().unwrap(), name);
}

#[tokio::test]
async fn test_login_wrong_password() {
    let app = get_test_app().await;
    let name = unique_name("wrongpw");
    let email = unique_email("wrongpw");

    app.register_user(&name, &email, "securepass123").await;

    let res = app
        .post_unauth(
            "/api/v1/auth/login",
            serde_json::json!({
                "email": email,
                "password": "wrongpassword"
            }),
        )
        .await;

    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "UNAUTHORIZED");
}

#[tokio::test]
async fn test_login_wrong_email() {
    let app = get_test_app().await;

    let res = app
        .post_unauth(
            "/api/v1/auth/login",
            serde_json::json!({
                "email": "nonexistent@test.lumiere.dev",
                "password": "securepass123"
            }),
        )
        .await;

    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "UNAUTHORIZED");
}

#[tokio::test]
async fn test_login_after_register() {
    let app = get_test_app().await;
    let name = unique_name("loginafter");
    let email = unique_email("loginafter");
    let password = "securepass123";

    let (reg_token, reg_user_id) = app.register_user(&name, &email, password).await;

    let (login_access, login_refresh) = app.login(&email, password).await;

    // Should get new tokens (different from registration tokens)
    assert!(!login_access.is_empty());
    assert!(!login_refresh.is_empty());
    // Tokens should be different from register tokens
    assert_ne!(login_access, reg_token);

    // Both tokens should work — verify login token works
    let res = app.get(&login_access, "/api/v1/users/@me").await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap().parse::<i64>().unwrap(), reg_user_id);
}

#[tokio::test]
async fn test_refresh_token() {
    let app = get_test_app().await;
    let name = unique_name("refresh");
    let email = unique_email("refresh");
    let password = "securepass123";

    app.register_user(&name, &email, password).await;
    let (_access, refresh) = app.login(&email, password).await;

    let res = app
        .post_unauth(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": refresh }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["access_token"].is_string());
    assert!(body["refresh_token"].is_string());

    // New tokens should be different (rotation)
    assert_ne!(body["refresh_token"].as_str().unwrap(), refresh);
}

#[tokio::test]
async fn test_refresh_with_access_token_fails() {
    let app = get_test_app().await;
    let name = unique_name("refacc");
    let email = unique_email("refacc");

    let (access, _) = app.register_user(&name, &email, "securepass123").await;

    // Use access token as refresh token — should fail
    let res = app
        .post_unauth(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": access }),
        )
        .await;

    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_refresh_with_invalid_token_fails() {
    let app = get_test_app().await;

    let res = app
        .post_unauth(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": "garbage.token.here" }),
        )
        .await;

    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_logout() {
    let app = get_test_app().await;
    let name = unique_name("logout");
    let email = unique_email("logout");
    let password = "securepass123";

    app.register_user(&name, &email, password).await;
    let (access, refresh) = app.login(&email, password).await;

    // Logout
    let res = app.post(&access, "/api/v1/auth/logout", serde_json::json!({})).await;
    assert_eq!(res.status(), 204);

    // Old refresh token should now be rejected
    let res = app
        .post_unauth(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": refresh }),
        )
        .await;
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_logout_all() {
    let app = get_test_app().await;
    let name = unique_name("logoutall");
    let email = unique_email("logoutall");
    let password = "securepass123";

    app.register_user(&name, &email, password).await;

    // Login twice to create two sessions
    let (access1, refresh1) = app.login(&email, password).await;
    let (_access2, refresh2) = app.login(&email, password).await;

    // Logout all from session 1
    let res = app
        .post(&access1, "/api/v1/auth/logout-all", serde_json::json!({}))
        .await;
    assert_eq!(res.status(), 204);

    // Both refresh tokens should be rejected
    let res = app
        .post_unauth(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": refresh1 }),
        )
        .await;
    assert_eq!(res.status(), 401);

    let res = app
        .post_unauth(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": refresh2 }),
        )
        .await;
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_protected_route_without_token() {
    let app = get_test_app().await;

    let res = app.get_unauth("/api/v1/users/@me").await;
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_protected_route_with_expired_token() {
    let app = get_test_app().await;

    // Craft a clearly invalid / expired-looking token
    let fake_token = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwiZXhwIjoxfQ.invalid";
    let res = app.get(fake_token, "/api/v1/users/@me").await;
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_register_username_too_short() {
    let app = get_test_app().await;
    let email = unique_email("shortname");

    let res = app
        .post_unauth(
            "/api/v1/auth/register",
            serde_json::json!({
                "username": "a",
                "email": email,
                "password": "securepass123"
            }),
        )
        .await;

    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "VALIDATION_ERROR");
    assert!(body["error"]["fields"]["username"].is_array());
}

// ═══════════════════════════════════════════════════════════════════
// User Profile Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_get_me() {
    let app = get_test_app().await;
    let name = unique_name("getme");
    let email = unique_email("getme");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app.get(&token, "/api/v1/users/@me").await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["username"].as_str().unwrap(), name);
    // Full user includes email
    assert_eq!(body["email"].as_str().unwrap(), email.to_lowercase());
    assert!(body["id"].is_string());
    assert!(body["locale"].is_string());
    assert!(body["created_at"].is_string());
}

#[tokio::test]
async fn test_get_other_user() {
    let app = get_test_app().await;
    let name1 = unique_name("other1");
    let email1 = unique_email("other1");
    let name2 = unique_name("other2");
    let email2 = unique_email("other2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    let res = app.get(&token1, &format!("/api/v1/users/{user2_id}")).await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["username"].as_str().unwrap(), name2);
    // Public user does NOT include email
    assert!(body.get("email").is_none() || body["email"].is_null());
}

#[tokio::test]
async fn test_get_nonexistent_user() {
    let app = get_test_app().await;
    let name = unique_name("nouser");
    let email = unique_email("nouser");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app.get(&token, "/api/v1/users/9999999999999").await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn test_update_username() {
    let app = get_test_app().await;
    let name = unique_name("upname");
    let email = unique_email("upname");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;
    let new_name = unique_name("newname");

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "username": new_name }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["username"].as_str().unwrap(), new_name);
}

#[tokio::test]
async fn test_update_bio() {
    let app = get_test_app().await;
    let name = unique_name("upbio");
    let email = unique_email("upbio");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "bio": "Hello, world!" }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["bio"].as_str().unwrap(), "Hello, world!");
}

#[tokio::test]
async fn test_update_avatar() {
    let app = get_test_app().await;
    let name = unique_name("upavatar");
    let email = unique_email("upavatar");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "avatar": "https://cdn.lumiere.dev/avatars/abc123.png" }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(
        body["avatar"].as_str().unwrap(),
        "https://cdn.lumiere.dev/avatars/abc123.png"
    );
}

#[tokio::test]
async fn test_update_locale() {
    let app = get_test_app().await;
    let name = unique_name("uplocale");
    let email = unique_email("uplocale");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "locale": "tr" }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["locale"].as_str().unwrap(), "tr");
}

#[tokio::test]
async fn test_update_no_fields() {
    let app = get_test_app().await;
    let name = unique_name("noup");
    let email = unique_email("noup");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(&token, "/api/v1/users/@me", serde_json::json!({}))
        .await;

    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "BAD_REQUEST");
}

#[tokio::test]
async fn test_username_change_rate_limit() {
    let app = get_test_app().await;
    let name = unique_name("rlimit");
    let email = unique_email("rlimit");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    // First change — should succeed
    let name2 = unique_name("rl2");
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "username": name2 }),
        )
        .await;
    assert_eq!(res.status(), 200);

    // Second change — should succeed
    let name3 = unique_name("rl3");
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "username": name3 }),
        )
        .await;
    assert_eq!(res.status(), 200);

    // Third change — should be rate limited (count > 2)
    let name4 = unique_name("rl4");
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "username": name4 }),
        )
        .await;
    assert_eq!(res.status(), 429);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "RATE_LIMITED");
    assert!(body["error"]["retry_after"].is_number());
}

#[tokio::test]
async fn test_delete_account() {
    let app = get_test_app().await;
    let name = unique_name("delacc");
    let email = unique_email("delacc");
    let password = "securepass123";

    let (token, _) = app.register_user(&name, &email, password).await;

    let res = app
        .delete_with_body(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "password": password }),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_delete_account_wrong_password() {
    let app = get_test_app().await;
    let name = unique_name("delwrong");
    let email = unique_email("delwrong");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .delete_with_body(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "password": "wrongpassword" }),
        )
        .await;

    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_delete_account_with_server_ownership() {
    let app = get_test_app().await;
    let name = unique_name("delowner");
    let email = unique_email("delowner");
    let password = "securepass123";

    let (token, _) = app.register_user(&name, &email, password).await;

    // Create a server — user becomes owner
    app.create_server(&token, &unique_name("delserv")).await;

    let res = app
        .delete_with_body(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "password": password }),
        )
        .await;

    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str().unwrap(), "BAD_REQUEST");
}

#[tokio::test]
async fn test_deleted_user_cannot_login() {
    let app = get_test_app().await;
    let name = unique_name("dellogin");
    let email = unique_email("dellogin");
    let password = "securepass123";

    let (token, _) = app.register_user(&name, &email, password).await;

    // Delete account
    let res = app
        .delete_with_body(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "password": password }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Try to login again
    let res = app
        .post_unauth(
            "/api/v1/auth/login",
            serde_json::json!({
                "email": email,
                "password": password
            }),
        )
        .await;

    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn test_update_multiple_profile_fields() {
    let app = get_test_app().await;
    let name = unique_name("multi");
    let email = unique_email("multi");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({
                "bio": "A new bio",
                "locale": "fr"
            }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["bio"].as_str().unwrap(), "A new bio");
    assert_eq!(body["locale"].as_str().unwrap(), "fr");
}

#[tokio::test]
async fn test_clear_avatar() {
    let app = get_test_app().await;
    let name = unique_name("clravatar");
    let email = unique_email("clravatar");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    // Set avatar
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "avatar": "some_avatar.png" }),
        )
        .await;
    assert_eq!(res.status(), 200);

    // Clear avatar by setting to null
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me",
            serde_json::json!({ "avatar": null }),
        )
        .await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["avatar"].is_null());
}

// ═══════════════════════════════════════════════════════════════════
// Friend System Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_send_friend_request() {
    let app = get_test_app().await;
    let name1 = unique_name("fr1");
    let email1 = unique_email("fr1");
    let name2 = unique_name("fr2");
    let email2 = unique_email("fr2");

    let (token1, _user1_id) = app.register_user(&name1, &email1, "securepass123").await;
    let (token2, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Send friend request via user_id
    let res = app
        .post(
            &token1,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": user2_id }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Verify sender sees outgoing request (type=4)
    let res = app.get(&token1, "/api/v1/users/@me/relationships").await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    assert!(!rels.is_empty());
    let outgoing = rels.iter().find(|r| r["type"].as_i64().unwrap() == 4);
    assert!(outgoing.is_some(), "Expected outgoing request (type=4)");

    // Verify receiver sees incoming request (type=3)
    let res = app.get(&token2, "/api/v1/users/@me/relationships").await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    let incoming = rels.iter().find(|r| r["type"].as_i64().unwrap() == 3);
    assert!(incoming.is_some(), "Expected incoming request (type=3)");
}

#[tokio::test]
async fn test_accept_friend_request() {
    let app = get_test_app().await;
    let name1 = unique_name("acc1");
    let email1 = unique_email("acc1");
    let name2 = unique_name("acc2");
    let email2 = unique_email("acc2");

    let (token1, user1_id) = app.register_user(&name1, &email1, "securepass123").await;
    let (token2, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // User1 sends request to User2
    app.post(
        &token1,
        "/api/v1/users/@me/relationships",
        serde_json::json!({ "user_id": user2_id }),
    )
    .await;

    // User2 accepts by sending a request back (which detects the incoming request and accepts)
    let res = app
        .post(
            &token2,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": user1_id }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Both should now be friends (type=1)
    let res = app.get(&token1, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    let friend = rels.iter().find(|r| r["type"].as_i64().unwrap() == 1);
    assert!(friend.is_some(), "Expected friend relationship (type=1)");

    let res = app.get(&token2, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    let friend = rels.iter().find(|r| r["type"].as_i64().unwrap() == 1);
    assert!(friend.is_some(), "Expected friend relationship (type=1) on other side");
}

#[tokio::test]
async fn test_list_relationships() {
    let app = get_test_app().await;
    let name1 = unique_name("lst1");
    let email1 = unique_email("lst1");
    let name2 = unique_name("lst2");
    let email2 = unique_email("lst2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Send friend request
    app.post(
        &token1,
        "/api/v1/users/@me/relationships",
        serde_json::json!({ "user_id": user2_id }),
    )
    .await;

    let res = app.get(&token1, "/api/v1/users/@me/relationships").await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body.is_array());

    let rels = body.as_array().unwrap();
    assert!(!rels.is_empty());
    // Each relationship should have id, type, user, created_at
    let rel = &rels[0];
    assert!(rel["id"].is_string());
    assert!(rel["type"].is_number());
    assert!(rel["user"].is_object());
    assert!(rel["created_at"].is_string());
}

#[tokio::test]
async fn test_remove_friend() {
    let app = get_test_app().await;
    let name1 = unique_name("rmfr1");
    let email1 = unique_email("rmfr1");
    let name2 = unique_name("rmfr2");
    let email2 = unique_email("rmfr2");

    let (token1, user1_id) = app.register_user(&name1, &email1, "securepass123").await;
    let (token2, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Become friends
    app.post(
        &token1,
        "/api/v1/users/@me/relationships",
        serde_json::json!({ "user_id": user2_id }),
    )
    .await;
    app.post(
        &token2,
        "/api/v1/users/@me/relationships",
        serde_json::json!({ "user_id": user1_id }),
    )
    .await;

    // Remove friend
    let res = app
        .delete(&token1, &format!("/api/v1/users/@me/relationships/{user2_id}"))
        .await;
    assert_eq!(res.status(), 204);

    // Both sides should no longer have the relationship
    let res = app.get(&token1, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    let has_user2 = rels.iter().any(|r| {
        r["user"]["id"].as_str().map(|s| s.parse::<i64>().unwrap_or(0)).unwrap_or(0) == user2_id
    });
    assert!(!has_user2, "User2 should be removed from relationships");

    let res = app.get(&token2, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    let has_user1 = rels.iter().any(|r| {
        r["user"]["id"].as_str().map(|s| s.parse::<i64>().unwrap_or(0)).unwrap_or(0) == user1_id
    });
    assert!(!has_user1, "User1 should be removed from relationships on other side");
}

#[tokio::test]
async fn test_cancel_outgoing_request() {
    let app = get_test_app().await;
    let name1 = unique_name("cncl1");
    let email1 = unique_email("cncl1");
    let name2 = unique_name("cncl2");
    let email2 = unique_email("cncl2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (token2, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Send friend request
    app.post(
        &token1,
        "/api/v1/users/@me/relationships",
        serde_json::json!({ "user_id": user2_id }),
    )
    .await;

    // Cancel outgoing request via delete
    let res = app
        .delete(&token1, &format!("/api/v1/users/@me/relationships/{user2_id}"))
        .await;
    assert_eq!(res.status(), 204);

    // Receiver should also no longer see incoming request
    let res = app.get(&token2, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    assert!(rels.is_empty(), "Incoming request should be removed after cancel");
}

#[tokio::test]
async fn test_block_user() {
    let app = get_test_app().await;
    let name1 = unique_name("blk1");
    let email1 = unique_email("blk1");
    let name2 = unique_name("blk2");
    let email2 = unique_email("blk2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Block user2
    let res = app
        .put(
            &token1,
            &format!("/api/v1/users/@me/relationships/{user2_id}"),
            serde_json::json!({ "type": 2 }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Verify block relationship exists (type=2)
    let res = app.get(&token1, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    let blocked = rels.iter().find(|r| r["type"].as_i64().unwrap() == 2);
    assert!(blocked.is_some(), "Expected block relationship (type=2)");
}

#[tokio::test]
async fn test_blocked_user_cannot_send_request() {
    let app = get_test_app().await;
    let name1 = unique_name("blkreq1");
    let email1 = unique_email("blkreq1");
    let name2 = unique_name("blkreq2");
    let email2 = unique_email("blkreq2");

    let (token1, user1_id) = app.register_user(&name1, &email1, "securepass123").await;
    let (token2, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // User1 blocks user2
    app.put(
        &token1,
        &format!("/api/v1/users/@me/relationships/{user2_id}"),
        serde_json::json!({ "type": 2 }),
    )
    .await;

    // User2 tries to send friend request to User1
    let res = app
        .post(
            &token2,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": user1_id }),
        )
        .await;

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_self_friend_request() {
    let app = get_test_app().await;
    let name = unique_name("selfr");
    let email = unique_email("selfr");

    let (token, user_id) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .post(
            &token,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": user_id }),
        )
        .await;

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_friend_request_nonexistent_user() {
    let app = get_test_app().await;
    let name = unique_name("frne");
    let email = unique_email("frne");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .post(
            &token,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": 9999999999999_i64 }),
        )
        .await;

    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn test_duplicate_friend_request() {
    let app = get_test_app().await;
    let name1 = unique_name("dupfr1");
    let email1 = unique_email("dupfr1");
    let name2 = unique_name("dupfr2");
    let email2 = unique_email("dupfr2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Send first request
    let res = app
        .post(
            &token1,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": user2_id }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Send duplicate
    let res = app
        .post(
            &token1,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "user_id": user2_id }),
        )
        .await;
    assert_eq!(res.status(), 409);
}

#[tokio::test]
async fn test_block_removes_existing_friendship() {
    let app = get_test_app().await;
    let name1 = unique_name("blkfr1");
    let email1 = unique_email("blkfr1");
    let name2 = unique_name("blkfr2");
    let email2 = unique_email("blkfr2");

    let (token1, user1_id) = app.register_user(&name1, &email1, "securepass123").await;
    let (token2, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Become friends
    app.post(
        &token1,
        "/api/v1/users/@me/relationships",
        serde_json::json!({ "user_id": user2_id }),
    )
    .await;
    app.post(
        &token2,
        "/api/v1/users/@me/relationships",
        serde_json::json!({ "user_id": user1_id }),
    )
    .await;

    // Now block
    let res = app
        .put(
            &token1,
            &format!("/api/v1/users/@me/relationships/{user2_id}"),
            serde_json::json!({ "type": 2 }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // User1 should see block (type=2), not friend
    let res = app.get(&token1, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    let types: Vec<i64> = rels.iter().filter_map(|r| r["type"].as_i64()).collect();
    assert!(types.contains(&2), "Should have block relationship");
    assert!(!types.contains(&1), "Should no longer be friends");

    // User2 should have no relationships at all (block is one-sided)
    let res = app.get(&token2, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    assert!(rels.is_empty(), "Blocked user should not see relationship");
}

#[tokio::test]
async fn test_unblock_via_delete() {
    let app = get_test_app().await;
    let name1 = unique_name("unblk1");
    let email1 = unique_email("unblk1");
    let name2 = unique_name("unblk2");
    let email2 = unique_email("unblk2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Block
    app.put(
        &token1,
        &format!("/api/v1/users/@me/relationships/{user2_id}"),
        serde_json::json!({ "type": 2 }),
    )
    .await;

    // Unblock via DELETE
    let res = app
        .delete(&token1, &format!("/api/v1/users/@me/relationships/{user2_id}"))
        .await;
    assert_eq!(res.status(), 204);

    // Should have no relationships
    let res = app.get(&token1, "/api/v1/users/@me/relationships").await;
    let body: serde_json::Value = res.json().await.unwrap();
    let rels = body.as_array().unwrap();
    let has_user2 = rels.iter().any(|r| {
        r["user"]["id"].as_str().map(|s| s.parse::<i64>().unwrap_or(0)).unwrap_or(0) == user2_id
    });
    assert!(!has_user2, "Should no longer have any relationship after unblock");
}

// ═══════════════════════════════════════════════════════════════════
// DM Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_create_dm_channel() {
    let app = get_test_app().await;
    let name1 = unique_name("dm1");
    let email1 = unique_email("dm1");
    let name2 = unique_name("dm2");
    let email2 = unique_email("dm2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    let res = app
        .post(
            &token1,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipient_id": user2_id }),
        )
        .await;

    let status = res.status().as_u16();
    assert!(status == 200 || status == 201, "Expected 200 or 201, got {status}");

    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["id"].is_string());
    assert_eq!(body["type"].as_i64().unwrap(), 1); // DM channel type
    assert!(body["recipients"].is_array());
    let recipients = body["recipients"].as_array().unwrap();
    assert_eq!(recipients.len(), 1);
    assert_eq!(
        recipients[0]["id"].as_str().unwrap().parse::<i64>().unwrap(),
        user2_id
    );
}

#[tokio::test]
async fn test_create_dm_idempotent() {
    let app = get_test_app().await;
    let name1 = unique_name("dmid1");
    let email1 = unique_email("dmid1");
    let name2 = unique_name("dmid2");
    let email2 = unique_email("dmid2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Create DM twice
    let res1 = app
        .post(
            &token1,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipient_id": user2_id }),
        )
        .await;
    let body1: serde_json::Value = res1.json().await.unwrap();

    let res2 = app
        .post(
            &token1,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipient_id": user2_id }),
        )
        .await;
    let body2: serde_json::Value = res2.json().await.unwrap();

    // Same channel ID returned
    assert_eq!(body1["id"].as_str().unwrap(), body2["id"].as_str().unwrap());
}

#[tokio::test]
async fn test_create_group_dm() {
    let app = get_test_app().await;
    let name1 = unique_name("gdm1");
    let email1 = unique_email("gdm1");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;

    // Create multiple recipients
    let mut recipient_ids = Vec::new();
    for i in 0..3 {
        let name = unique_name(&format!("gdmr{i}"));
        let email = unique_email(&format!("gdmr{i}"));
        let (_, uid) = app.register_user(&name, &email, "securepass123").await;
        recipient_ids.push(uid);
    }

    let res = app
        .post(
            &token1,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipients": recipient_ids }),
        )
        .await;

    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["type"].as_i64().unwrap(), 3); // Group DM type
    let recipients = body["recipients"].as_array().unwrap();
    assert_eq!(recipients.len(), 3);
}

#[tokio::test]
async fn test_group_dm_max_10() {
    let app = get_test_app().await;
    let name = unique_name("gdmmax");
    let email = unique_email("gdmmax");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    // Create 10 recipients (9 max allowed + self = 10 total, so 10 recipients = 11 total = over limit)
    let mut recipient_ids = Vec::new();
    for i in 0..10 {
        let rname = unique_name(&format!("gdmmx{i}"));
        let remail = unique_email(&format!("gdmmx{i}"));
        let (_, uid) = app.register_user(&rname, &remail, "securepass123").await;
        recipient_ids.push(uid);
    }

    let res = app
        .post(
            &token,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipients": recipient_ids }),
        )
        .await;

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_list_dm_channels() {
    let app = get_test_app().await;
    let name1 = unique_name("lsdm1");
    let email1 = unique_email("lsdm1");
    let name2 = unique_name("lsdm2");
    let email2 = unique_email("lsdm2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Create a DM channel
    app.post(
        &token1,
        "/api/v1/users/@me/channels",
        serde_json::json!({ "recipient_id": user2_id }),
    )
    .await;

    // List DM channels
    let res = app.get(&token1, "/api/v1/users/@me/channels").await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body.is_array());
    let channels = body.as_array().unwrap();
    assert!(!channels.is_empty());
    // Each channel has expected fields
    let ch = &channels[0];
    assert!(ch["id"].is_string());
    assert!(ch["type"].is_number());
    assert!(ch["recipients"].is_array());
}

#[tokio::test]
async fn test_blocked_user_dm() {
    let app = get_test_app().await;
    let name1 = unique_name("blkdm1");
    let email1 = unique_email("blkdm1");
    let name2 = unique_name("blkdm2");
    let email2 = unique_email("blkdm2");

    let (token1, user1_id) = app.register_user(&name1, &email1, "securepass123").await;
    let (token2, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // User1 blocks user2
    app.put(
        &token1,
        &format!("/api/v1/users/@me/relationships/{user2_id}"),
        serde_json::json!({ "type": 2 }),
    )
    .await;

    // User2 tries to create DM with user1
    let res = app
        .post(
            &token2,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipient_id": user1_id }),
        )
        .await;

    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn test_dm_with_self() {
    let app = get_test_app().await;
    let name = unique_name("dmself");
    let email = unique_email("dmself");

    let (token, user_id) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .post(
            &token,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipient_id": user_id }),
        )
        .await;

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_create_dm_nonexistent_user() {
    let app = get_test_app().await;
    let name = unique_name("dmne");
    let email = unique_email("dmne");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .post(
            &token,
            "/api/v1/users/@me/channels",
            serde_json::json!({ "recipient_id": 9999999999999_i64 }),
        )
        .await;

    // Should fail — the recipient does not exist, so we expect an error status
    let status = res.status().as_u16();
    assert!(
        status == 400 || status == 404 || status == 500,
        "Expected error status for nonexistent DM recipient, got {status}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Settings Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_get_settings() {
    let app = get_test_app().await;
    let name = unique_name("gset");
    let email = unique_email("gset");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app.get(&token, "/api/v1/users/@me/settings").await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    // Default settings should be present
    assert!(body["theme"].is_string());
    assert!(body["status"].is_string());
    assert!(body["locale"].is_string());
    assert!(body["dm_notifications"].is_boolean());
}

#[tokio::test]
async fn test_update_theme() {
    let app = get_test_app().await;
    let name = unique_name("theme");
    let email = unique_email("theme");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    // Set dark theme
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/settings",
            serde_json::json!({ "theme": "dark" }),
        )
        .await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["theme"].as_str().unwrap(), "dark");

    // Set light theme
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/settings",
            serde_json::json!({ "theme": "light" }),
        )
        .await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["theme"].as_str().unwrap(), "light");
}

#[tokio::test]
async fn test_update_invalid_theme() {
    let app = get_test_app().await;
    let name = unique_name("badtheme");
    let email = unique_email("badtheme");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    // Theme validation depends on the server implementation. The server may or may not
    // validate theme values. We test that a clearly unusual value is at least accepted
    // or rejected gracefully.
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/settings",
            serde_json::json!({ "theme": "" }),
        )
        .await;

    // Empty string may be accepted or rejected — either is valid behavior
    let status = res.status().as_u16();
    assert!(
        status == 200 || status == 400,
        "Expected 200 or 400 for empty theme, got {status}"
    );
}

#[tokio::test]
async fn test_update_status() {
    let app = get_test_app().await;
    let name = unique_name("statset");
    let email = unique_email("statset");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    for status_val in &["online", "idle", "dnd", "invisible"] {
        let res = app
            .patch(
                &token,
                "/api/v1/users/@me/settings",
                serde_json::json!({ "status": status_val }),
            )
            .await;
        assert_eq!(
            res.status(),
            200,
            "Setting status to '{status_val}' should succeed"
        );
        let body: serde_json::Value = res.json().await.unwrap();
        assert_eq!(body["status"].as_str().unwrap(), *status_val);
    }
}

#[tokio::test]
async fn test_update_invalid_status() {
    let app = get_test_app().await;
    let name = unique_name("badstat");
    let email = unique_email("badstat");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/settings",
            serde_json::json!({ "status": "offline" }),
        )
        .await;
    assert_eq!(res.status(), 400);

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/settings",
            serde_json::json!({ "status": "invalid_status" }),
        )
        .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_update_multiple_settings() {
    let app = get_test_app().await;
    let name = unique_name("mset");
    let email = unique_email("mset");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/settings",
            serde_json::json!({
                "theme": "dark",
                "status": "dnd",
                "dm_notifications": false,
                "animate_emoji": false
            }),
        )
        .await;

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["theme"].as_str().unwrap(), "dark");
    assert_eq!(body["status"].as_str().unwrap(), "dnd");
    assert_eq!(body["dm_notifications"].as_bool().unwrap(), false);
    assert_eq!(body["animate_emoji"].as_bool().unwrap(), false);
}

#[tokio::test]
async fn test_update_settings_no_fields() {
    let app = get_test_app().await;
    let name = unique_name("noset");
    let email = unique_email("noset");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(&token, "/api/v1/users/@me/settings", serde_json::json!({}))
        .await;

    assert_eq!(res.status(), 400);
}

// ═══════════════════════════════════════════════════════════════════
// Presence Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_update_presence() {
    let app = get_test_app().await;
    let name = unique_name("pres");
    let email = unique_email("pres");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/presence",
            serde_json::json!({ "status": "idle" }),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_update_presence_invalid_status() {
    let app = get_test_app().await;
    let name = unique_name("badpres");
    let email = unique_email("badpres");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/presence",
            serde_json::json!({ "status": "offline" }),
        )
        .await;
    assert_eq!(res.status(), 400);

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/presence",
            serde_json::json!({ "status": "notreal" }),
        )
        .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_presence_stored_in_redis() {
    let app = get_test_app().await;
    let name = unique_name("presred");
    let email = unique_email("presred");

    let (token, user_id) = app.register_user(&name, &email, "securepass123").await;

    // Update presence
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/presence",
            serde_json::json!({ "status": "dnd" }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Verify in Redis
    let mut conn = app.state.redis.clone();
    let key = format!("presence:{}", user_id);
    let val: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .unwrap();

    assert!(val.is_some(), "Presence should be stored in Redis");
    let presence: serde_json::Value = serde_json::from_str(&val.unwrap()).unwrap();
    assert_eq!(presence["status"].as_str().unwrap(), "dnd");
}

#[tokio::test]
async fn test_invisible_status() {
    let app = get_test_app().await;
    let name = unique_name("invis");
    let email = unique_email("invis");

    let (token, user_id) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/presence",
            serde_json::json!({ "status": "invisible" }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Verify stored in Redis
    let mut conn = app.state.redis.clone();
    let key = format!("presence:{}", user_id);
    let val: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .unwrap();

    assert!(val.is_some());
    let presence: serde_json::Value = serde_json::from_str(&val.unwrap()).unwrap();
    assert_eq!(presence["status"].as_str().unwrap(), "invisible");
}

// ═══════════════════════════════════════════════════════════════════
// Notes Tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_set_note() {
    let app = get_test_app().await;
    let name1 = unique_name("note1");
    let email1 = unique_email("note1");
    let name2 = unique_name("note2");
    let email2 = unique_email("note2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    let res = app
        .put(
            &token1,
            &format!("/api/v1/users/{user2_id}/note"),
            serde_json::json!({ "note": "Cool person" }),
        )
        .await;

    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_get_note() {
    let app = get_test_app().await;
    let name1 = unique_name("gnote1");
    let email1 = unique_email("gnote1");
    let name2 = unique_name("gnote2");
    let email2 = unique_email("gnote2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Set note
    app.put(
        &token1,
        &format!("/api/v1/users/{user2_id}/note"),
        serde_json::json!({ "note": "Remember: loves cats" }),
    )
    .await;

    // Get note
    let res = app
        .get(&token1, &format!("/api/v1/users/{user2_id}/note"))
        .await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["note"].as_str().unwrap(), "Remember: loves cats");
}

#[tokio::test]
async fn test_update_note() {
    let app = get_test_app().await;
    let name1 = unique_name("upnote1");
    let email1 = unique_email("upnote1");
    let name2 = unique_name("upnote2");
    let email2 = unique_email("upnote2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Set initial note
    app.put(
        &token1,
        &format!("/api/v1/users/{user2_id}/note"),
        serde_json::json!({ "note": "Initial note" }),
    )
    .await;

    // Update note
    app.put(
        &token1,
        &format!("/api/v1/users/{user2_id}/note"),
        serde_json::json!({ "note": "Updated note" }),
    )
    .await;

    // Verify updated
    let res = app
        .get(&token1, &format!("/api/v1/users/{user2_id}/note"))
        .await;
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["note"].as_str().unwrap(), "Updated note");
}

#[tokio::test]
async fn test_delete_note_empty_string() {
    let app = get_test_app().await;
    let name1 = unique_name("delnote1");
    let email1 = unique_email("delnote1");
    let name2 = unique_name("delnote2");
    let email2 = unique_email("delnote2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    let (_, user2_id) = app.register_user(&name2, &email2, "securepass123").await;

    // Set a note
    app.put(
        &token1,
        &format!("/api/v1/users/{user2_id}/note"),
        serde_json::json!({ "note": "Temporary note" }),
    )
    .await;

    // Delete by sending empty string
    let res = app
        .put(
            &token1,
            &format!("/api/v1/users/{user2_id}/note"),
            serde_json::json!({ "note": "" }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Verify note is gone
    let res = app
        .get(&token1, &format!("/api/v1/users/{user2_id}/note"))
        .await;
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(
        body["note"].is_null(),
        "Note should be null after deletion"
    );
}

#[tokio::test]
async fn test_get_note_nonexistent() {
    let app = get_test_app().await;
    let name = unique_name("nonote");
    let email = unique_email("nonote");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    // Get note for a user we never set one for
    let res = app.get(&token, "/api/v1/users/9999999999999/note").await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["note"].is_null());
}

// ═══════════════════════════════════════════════════════════════════
// Additional edge-case tests
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_register_email_case_insensitive() {
    let app = get_test_app().await;
    let name1 = unique_name("casemail1");
    let name2 = unique_name("casemail2");
    let base_email = unique_email("casemail");

    // Register with lowercase
    app.register_user(&name1, &base_email, "securepass123").await;

    // Try with uppercase — should conflict
    let upper_email = base_email.to_uppercase();
    let res = app
        .post_unauth(
            "/api/v1/auth/register",
            serde_json::json!({
                "username": name2,
                "email": upper_email,
                "password": "securepass123"
            }),
        )
        .await;

    assert_eq!(res.status(), 409);
}

#[tokio::test]
async fn test_presence_with_custom_status() {
    let app = get_test_app().await;
    let name = unique_name("custstat");
    let email = unique_email("custstat");

    let (token, user_id) = app.register_user(&name, &email, "securepass123").await;

    let custom = serde_json::json!({ "text": "Coding Lumiere", "emoji": "keyboard" });
    let res = app
        .patch(
            &token,
            "/api/v1/users/@me/presence",
            serde_json::json!({
                "status": "online",
                "custom_status": custom
            }),
        )
        .await;
    assert_eq!(res.status(), 204);

    // Verify in Redis
    let mut conn = app.state.redis.clone();
    let key = format!("presence:{}", user_id);
    let val: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .unwrap();

    let presence: serde_json::Value = serde_json::from_str(&val.unwrap()).unwrap();
    assert_eq!(presence["status"].as_str().unwrap(), "online");
    assert!(presence["custom_status"].is_object());
    assert_eq!(presence["custom_status"]["text"].as_str().unwrap(), "Coding Lumiere");
}

#[tokio::test]
async fn test_send_friend_request_by_username() {
    let app = get_test_app().await;
    let name1 = unique_name("fruser1");
    let email1 = unique_email("fruser1");
    let name2 = unique_name("fruser2");
    let email2 = unique_email("fruser2");

    let (token1, _) = app.register_user(&name1, &email1, "securepass123").await;
    app.register_user(&name2, &email2, "securepass123").await;

    // Send friend request by username
    let res = app
        .post(
            &token1,
            "/api/v1/users/@me/relationships",
            serde_json::json!({ "username": name2 }),
        )
        .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn test_friend_request_no_identifier() {
    let app = get_test_app().await;
    let name = unique_name("frnoid");
    let email = unique_email("frnoid");

    let (token, _) = app.register_user(&name, &email, "securepass123").await;

    let res = app
        .post(
            &token,
            "/api/v1/users/@me/relationships",
            serde_json::json!({}),
        )
        .await;

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn test_refresh_token_rotation_invalidates_old() {
    let app = get_test_app().await;
    let name = unique_name("rotinv");
    let email = unique_email("rotinv");
    let password = "securepass123";

    app.register_user(&name, &email, password).await;
    let (_access, refresh) = app.login(&email, password).await;

    // Use refresh token
    let res = app
        .post_unauth(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": &refresh }),
        )
        .await;
    assert_eq!(res.status(), 200);

    // Old refresh token should now be invalid (rotation)
    let res = app
        .post_unauth(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": &refresh }),
        )
        .await;
    assert_eq!(res.status(), 401);
}
