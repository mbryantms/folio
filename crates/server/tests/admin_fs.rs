//! Integration coverage for `GET /admin/fs/list` — the directory picker
//! the Admin → New Library dialog uses.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

fn extract_cookie(resp: &Response<Body>, name: &str) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
}

async fn body_bytes(b: Body) -> Vec<u8> {
    to_bytes(b, usize::MAX).await.unwrap().to_vec()
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let session = extract_cookie(&resp, "__Host-comic_session");
    let csrf = extract_cookie(&resp, "__Host-comic_csrf");
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn demote_to_user(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = entity::user::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("user".into());
    am.update(&db).await.unwrap();
}

async fn get(app: &TestApp, auth: &Authed, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn rejects_non_admin() {
    let tmp = tempfile::tempdir().unwrap();
    let app = TestApp::spawn_with_library_root(tmp.path().to_path_buf()).await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    demote_to_user(&app, user.user_id).await;
    let (s, _) = get(&app, &user, "/admin/fs/list").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lists_root_when_path_omitted() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("alpha")).unwrap();
    std::fs::create_dir(tmp.path().join("beta")).unwrap();
    std::fs::write(tmp.path().join("ignored.txt"), b"not a directory").unwrap();
    std::fs::create_dir(tmp.path().join(".hidden")).unwrap();

    let app = TestApp::spawn_with_library_root(tmp.path().to_path_buf()).await;
    let admin = register(&app, "admin@example.com").await;

    let (s, body) = get(&app, &admin, "/admin/fs/list").await;
    assert_eq!(s, StatusCode::OK);

    let names: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["alpha", "beta"],
        "directories only, dotfiles + plain files filtered, sorted asc"
    );

    let root = body["root"].as_str().unwrap();
    let listed = body["path"].as_str().unwrap();
    assert_eq!(
        listed, root,
        "without `path`, the handler returns the library root"
    );
}

#[tokio::test]
async fn drills_into_a_subdirectory() {
    let tmp = tempfile::tempdir().unwrap();
    let series = tmp.path().join("marvel");
    std::fs::create_dir(&series).unwrap();
    std::fs::create_dir(series.join("uncanny-x-men")).unwrap();
    std::fs::create_dir(series.join("daredevil")).unwrap();

    let app = TestApp::spawn_with_library_root(tmp.path().to_path_buf()).await;
    let admin = register(&app, "admin@example.com").await;

    let uri = format!(
        "/admin/fs/list?path={}",
        urlencoding::encode(&series.to_string_lossy())
    );
    let (s, body) = get(&app, &admin, &uri).await;
    assert_eq!(s, StatusCode::OK);
    let names: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["daredevil", "uncanny-x-men"]);
}

#[tokio::test]
async fn rejects_path_outside_root() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let app = TestApp::spawn_with_library_root(tmp.path().to_path_buf()).await;
    let admin = register(&app, "admin@example.com").await;

    let uri = format!(
        "/admin/fs/list?path={}",
        urlencoding::encode(&outside.path().to_string_lossy())
    );
    let (s, body) = get(&app, &admin, &uri).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "must reject paths outside the configured root"
    );
    assert_eq!(body["error"]["code"], "forbidden");
}

#[tokio::test]
async fn rejects_parent_dir_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let app = TestApp::spawn_with_library_root(tmp.path().to_path_buf()).await;
    let admin = register(&app, "admin@example.com").await;

    // Even a `..` inside what would canonicalise to a valid path is
    // rejected early on the validation pass — defence in depth.
    let uri = format!(
        "/admin/fs/list?path={}",
        urlencoding::encode(&format!(
            "{}/../{}",
            tmp.path().display(),
            tmp.path().file_name().unwrap().to_string_lossy()
        ))
    );
    let (s, body) = get(&app, &admin, &uri).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test]
async fn missing_path_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let app = TestApp::spawn_with_library_root(tmp.path().to_path_buf()).await;
    let admin = register(&app, "admin@example.com").await;

    let nonexistent = tmp.path().join("does-not-exist");
    let uri = format!(
        "/admin/fs/list?path={}",
        urlencoding::encode(&nonexistent.to_string_lossy())
    );
    let (s, body) = get(&app, &admin, &uri).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}
