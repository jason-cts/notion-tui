use super::*;
use crate::{
    model::Kind,
    service::{NotionService, OAuthService, Write},
};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tempfile::TempDir;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

fn config() -> OAuthConfig {
    OAuthConfig {
        client_id: "client".into(),
        client_secret: "secret".into(),
        redirect: Url::parse(DEFAULT_REDIRECT).unwrap(),
    }
}

fn credentials(access: &str, refresh: &str) -> Credentials {
    Credentials {
        version: 1,
        client_id: "client".into(),
        access_token: access.into(),
        refresh_token: refresh.into(),
        workspace_id: "workspace".into(),
        workspace_name: Some("Workspace".into()),
        workspace_icon: None,
        bot_id: "bot".into(),
    }
}

fn token_response() -> Value {
    json!({"access_token":"new", "refresh_token":"r-new", "workspace_id":"workspace", "workspace_name":"Workspace", "bot_id":"bot"})
}

fn store() -> (TempDir, TokenStore) {
    let directory = TempDir::new().unwrap();
    let store = TokenStore::at(directory.path().join("notion-tui"));
    (directory, store)
}

async fn saved_store() -> (TempDir, TokenStore) {
    let (directory, store) = store();
    let _lock = store.lock().await.unwrap();
    store.save(&credentials("old", "r-old")).unwrap();
    (directory, store)
}

fn client(server: &MockServer) -> OAuthClient {
    let mut client = OAuthClient::new(config()).unwrap();
    client.api_base = server.uri();
    client
}

fn session(client: OAuthClient, store: TokenStore) -> OAuthSession {
    OAuthSession {
        client,
        store,
        credentials: Mutex::new(credentials("old", "r-old")),
    }
}

fn unauthorized() -> ResponseTemplate {
    ResponseTemplate::new(401).set_body_json(
        json!({"object":"error", "status":401,"code":"unauthorized","message":"Expired"}),
    )
}

async fn refresh_mock(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(header("Authorization", "Basic Y2xpZW50OnNlY3JldA=="))
        .and(body_json(
            json!({"grant_type":"refresh_token","refresh_token":"r-old"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_response()))
        .expect(1)
        .mount(server)
        .await;
}

#[test]
fn configuration_requires_secrets_and_validates_loopback_redirects() {
    let error = OAuthConfig::from_lookup(|_| Ok(None)).err().unwrap();
    assert!(error.to_string().contains("NOTION_OAUTH_CLIENT_ID"));
    let error = OAuthConfig::from_lookup(|name| {
        Ok((name == "NOTION_OAUTH_CLIENT_ID").then(|| "client".into()))
    })
    .err()
    .unwrap();
    assert!(error.to_string().contains("NOTION_OAUTH_CLIENT_SECRET"));
    for uri in [
        "https://example.com:8765/callback",
        "http://192.168.1.1:8765/callback",
        "http://localhost/callback",
        "http://127.0.0.1:0/callback",
        "http://user:secret@127.0.0.1:8765/callback",
        "http://localhost:8765/callback?code=x",
        "http://localhost:8765/callback#fragment",
    ] {
        let mut config = config();
        config.redirect = Url::parse(uri).unwrap();
        let error = config.callback_address().err().unwrap().to_string();
        assert!(!error.contains("secret"));
    }
    for uri in [
        DEFAULT_REDIRECT,
        "http://localhost:8888/callback",
        "http://[::1]:8888/callback",
    ] {
        let mut config = config();
        config.redirect = Url::parse(uri).unwrap();
        assert!(config.callback_address().unwrap().ip().is_loopback());
    }
}

#[test]
fn authorization_url_is_encoded_and_state_is_random() {
    let client = OAuthClient::new(config()).unwrap();
    let state = random_state().unwrap();
    assert_eq!(state.len(), 64);
    assert_ne!(state, random_state().unwrap());
    let url = client.authorization_url(&state).unwrap();
    let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(query["redirect_uri"], DEFAULT_REDIRECT);
    assert_eq!(query["state"], state);
    assert_eq!(query["client_id"], "client");
    assert_eq!(query["owner"], "user");
    assert_eq!(query["response_type"], "code");
    assert!(!url.as_str().contains("secret"));
}

#[tokio::test]
async fn code_exchange_uses_basic_auth_and_identical_redirect() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/oauth/token"))
        .and(header("Authorization", "Basic Y2xpZW50OnNlY3JldA=="))
        .and(header("Notion-Version", NOTION_API_VERSION))
        .and(body_json(json!({"grant_type":"authorization_code","code":"one-time-code","redirect_uri":DEFAULT_REDIRECT})))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_response())).expect(1).mount(&server).await;
    let result = client(&server).exchange(json!({"grant_type":"authorization_code", "code":"one-time-code", "redirect_uri":DEFAULT_REDIRECT})).await.unwrap();
    assert_eq!(result.access_token, "new");
    assert_eq!(result.refresh_token, "r-new");
}

#[tokio::test]
async fn callback_rejects_unrelated_and_untrusted_requests_before_accepting_once() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(callback::wait(
        listener,
        "/callback",
        "state",
        Duration::from_secs(5),
    ));
    let http = Client::new();
    for (path, status) in [
        ("/favicon.ico", 404),
        ("/callback?code=secret-code", 400),
        ("/callback?state=wrong&code=secret-code", 400),
        ("/callback?state=state&state=state&code=x", 400),
        ("/callback?state=state&code=x&code=y", 400),
        ("/callback?state=state", 400),
        ("/callback?error=access_denied", 400),
    ] {
        let response = http
            .get(format!("http://{address}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert!(!response.text().await.unwrap().contains("secret-code"));
        assert!(!task.is_finished());
    }
    let response = http
        .get(format!(
            "http://{address}/callback?state=state&code=secret-code"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.text().await.unwrap().contains("secret-code"));
    assert_eq!(task.await.unwrap().unwrap(), "secret-code");
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn callback_denial_timeout_and_cancellation_release_the_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(callback::wait(
        listener,
        "/callback",
        "state",
        Duration::from_secs(5),
    ));
    Client::new()
        .get(format!(
            "http://{address}/callback?state=state&error=access_denied"
        ))
        .send()
        .await
        .unwrap();
    assert!(matches!(
        task.await.unwrap().err().unwrap().downcast_ref(),
        Some(AuthError::Denied)
    ));
    let listener = TcpListener::bind(address).await.unwrap();
    let error = callback::wait(listener, "/callback", "state", Duration::from_millis(20))
        .await
        .err()
        .unwrap();
    assert!(matches!(error.downcast_ref(), Some(AuthError::Timeout)));
    let listener = TcpListener::bind(address).await.unwrap();
    let task = tokio::spawn(callback::wait(
        listener,
        "/callback",
        "state",
        Duration::from_secs(5),
    ));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(TcpListener::bind(address).await.is_ok());
}

#[tokio::test]
async fn saved_login_is_reused_without_refresh_or_browser_login() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    Mock::given(method("GET"))
        .and(path("/users/me"))
        .and(header("Authorization", "Bearer old"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let session = OAuthSession::connect_with(client(&server), store, false, |_| async {
        panic!("a valid saved token must not launch OAuth")
    })
    .await
    .unwrap();
    assert_eq!(session.token().await, "old");
}

#[tokio::test]
async fn expired_saved_login_refreshes_and_persists_both_rotated_tokens() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    Mock::given(method("GET"))
        .and(path("/users/me"))
        .and(header("Authorization", "Bearer old"))
        .respond_with(unauthorized())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/me"))
        .and(header("Authorization", "Bearer new"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    refresh_mock(&server).await;
    let restored = OAuthSession::restore(client(&server), store.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.token().await, "new");
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.access_token, "new");
    assert_eq!(saved.refresh_token, "r-new");
}

#[tokio::test]
async fn invalid_refresh_starts_new_login_and_force_login_bypasses_corrupt_cache() {
    let server = MockServer::start().await;
    let (directory, store) = saved_store().await;
    Mock::given(method("GET"))
        .and(path("/users/me"))
        .respond_with(unauthorized())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error":"invalid_grant"})))
        .expect(1)
        .mount(&server)
        .await;
    let logins = Arc::new(AtomicUsize::new(0));
    let count = logins.clone();
    let restored =
        OAuthSession::connect_with(client(&server), store.clone(), false, move |_| async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(credentials("reauthorized", "r-reauthorized"))
        })
        .await
        .unwrap();
    assert_eq!(restored.token().await, "reauthorized");
    assert_eq!(logins.load(Ordering::SeqCst), 1);
    fs::write(
        directory.path().join("notion-tui/auth.json"),
        b"bad json secret-value",
    )
    .unwrap();
    let restored = OAuthSession::connect_with(client(&server), store, true, |_| async {
        Ok(credentials("forced", "r-forced"))
    })
    .await
    .unwrap();
    assert_eq!(restored.token().await, "forced");
}

#[tokio::test]
async fn network_failure_preserves_credentials_and_never_opens_browser() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    Mock::given(method("GET"))
        .and(path("/users/me"))
        .respond_with(ResponseTemplate::new(503).set_body_string("secret-value"))
        .expect(1)
        .mount(&server)
        .await;
    let error = OAuthSession::connect_with(client(&server), store.clone(), false, |_| async {
        panic!("transient server failure must not launch OAuth")
    })
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("503"));
    assert!(!format!("{error:#}").contains("secret-value"));
    assert_eq!(store.load().unwrap().unwrap().access_token, "old");
}

#[tokio::test]
async fn refresh_failures_and_malformed_responses_do_not_replace_saved_credentials_or_leak_secrets()
{
    for (status, body) in [
        (503, json!({"message":"secret-value"})),
        (200, json!({"access_token":"secret-value"})),
    ] {
        let server = MockServer::start().await;
        let (_directory, store) = saved_store().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let session = session(client(&server), store.clone());
        let error = session.refresh("old").await.unwrap_err();
        assert!(!format!("{error:#}").contains("secret-value"));
        assert_eq!(session.token().await, "old");
        assert_eq!(store.load().unwrap().unwrap().refresh_token, "r-old");
    }
}

#[tokio::test]
async fn simultaneous_sessions_reload_rotated_credentials_under_the_file_lock() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    refresh_mock(&server).await;
    let first = session(client(&server), store.clone());
    let second = session(client(&server), store.clone());
    let (a, b, c) = tokio::join!(
        first.refresh("old"),
        second.refresh("old"),
        first.refresh("old")
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    assert_eq!(first.token().await, "new");
    assert_eq!(second.token().await, "new");
    assert_eq!(store.load().unwrap().unwrap().refresh_token, "r-new");
}

#[tokio::test]
async fn credentials_for_another_client_are_not_used() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    let mut other = credentials("other", "r-other");
    other.client_id = "different".into();
    store.save(&other).unwrap();
    assert!(
        OAuthSession::restore(client(&server), store)
            .await
            .unwrap()
            .is_none()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn store_atomically_replaces_private_credentials_and_logout_only_removes_local_login() {
    let (directory, store) = saved_store().await;
    let path = directory.path().join("notion-tui/auth.json");
    let before = fs::File::open(&path).unwrap();
    store.save(&credentials("new", "r-new")).unwrap();
    let old: Credentials = serde_json::from_reader(before).unwrap();
    assert_eq!(old.access_token, "old");
    assert_eq!(store.load().unwrap().unwrap().access_token, "new");
    assert!(!fs::read_to_string(&path).unwrap().contains("client_secret"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(path.with_file_name("auth.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    store.logout().await.unwrap();
    assert!(!path.exists());
    store.logout().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn store_rejects_symlinks_and_insecure_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (directory, store) = saved_store().await;
    let path = directory.path().join("notion-tui/auth.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(&credentials("new", "r-new")).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let real = directory.path().join("real.json");
    fs::rename(&path, &real).unwrap();
    symlink(&real, &path).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(&credentials("new", "r-new")).is_err());
    assert!(store.logout().await.is_err());
    assert!(real.exists());
    fs::remove_file(&path).unwrap();
    let lock = path.with_file_name("auth.lock");
    fs::remove_file(&lock).unwrap();
    symlink(&real, &lock).unwrap();
    assert!(store.lock().await.is_err());
    fs::remove_file(lock).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(store.lock().await.is_err());
}

#[tokio::test]
async fn runtime_reads_refresh_once_then_use_new_clients_for_subsequent_requests() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("Authorization", "Bearer old"))
        .respond_with(unauthorized())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("Authorization", "Bearer new"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"object":"list","results":[],"has_more":false,"next_cursor":null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/blocks/page/children"))
        .and(header("Authorization", "Bearer new"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"object":"list","results":[],"has_more":false,"next_cursor":null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    refresh_mock(&server).await;
    let api = OAuthService::new(session(client(&server), store))
        .await
        .unwrap();
    assert!(api.search("").await.unwrap().is_empty());
    assert!(api.children("page").await.unwrap().is_empty());
}

#[tokio::test]
async fn a_rejected_read_is_not_retried_indefinitely() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(unauthorized())
        .expect(2)
        .mount(&server)
        .await;
    refresh_mock(&server).await;
    let api = OAuthService::new(session(client(&server), store))
        .await
        .unwrap();
    let error = api.search("").await.unwrap_err();
    assert!(is_unauthorized(&error));
    assert!(error.to_string().contains("--login"));
}

#[tokio::test]
async fn rejected_writes_keep_app_drafts_and_are_only_retried_explicitly() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    Mock::given(method("PATCH"))
        .and(path("/blocks/page/children"))
        .and(header("Authorization", "Bearer old"))
        .respond_with(unauthorized())
        .expect(1)
        .mount(&server)
        .await;
    refresh_mock(&server).await;
    let api = Arc::new(
        OAuthService::new(session(client(&server), store))
            .await
            .unwrap(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = crate::app::App::new(api.clone(), tx, false);
    let mut editor = crate::editor::Editor::new(crate::editor::Target::Append("page".into()));
    editor.buffer.insert("unsaved draft");
    app.editor = Some(editor);
    app.key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('s'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    app.receive(
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap(),
    );
    assert!(!app.saving);
    assert_eq!(app.editor.as_ref().unwrap().buffer.text, "unsaved draft");
    assert!(
        app.editor
            .as_ref()
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .contains("not retried")
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.to_string() == "PATCH")
            .count(),
        1
    );
    Mock::given(method("PATCH"))
        .and(path("/blocks/page/children"))
        .and(header("Authorization", "Bearer new"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    api.write(Write::Append {
        parent: "page".into(),
        kind: Kind::Paragraph,
        text: "unsaved draft".into(),
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn logout_cannot_be_undone_by_a_stale_refresh() {
    let server = MockServer::start().await;
    let (_directory, store) = saved_store().await;
    let session = session(client(&server), store.clone());
    store.logout().await.unwrap();
    assert!(session.refresh("old").await.is_err());
    assert!(store.load().unwrap().is_none());
    assert!(server.received_requests().await.unwrap().is_empty());
}
