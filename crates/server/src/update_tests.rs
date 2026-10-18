use super::*;
use axum::http::{Method, Request};
use http_body_util::BodyExt;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[tokio::test]
async fn update_routes_require_session_and_csrf_and_explain_unconfigured_source() {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("test.db")).unwrap();
    db.execute(
        "INSERT INTO sessions VALUES(?,?,?)",
        params![digest("session-fixture"), "csrf-fixture", now() + 60000],
    )
    .unwrap();
    let app = App {
        db: Arc::new(Mutex::new(db)),
        origin: "https://panel.example.com".into(),
        secure: true,
        sockets: Default::default(),
        config_readers: Default::default(),
        attempts: Default::default(),
        downloads: dir.path().join("downloads"),
        updates: crate::updates::Service::new(None, None, dir.path().to_path_buf()).unwrap(),
        log_changes: tokio::sync::watch::channel(0).0,
        command_changes: tokio::sync::watch::channel(0).0,
    };
    let router = router(app);
    let request = |path: &str, method: Method, auth: bool, csrf: bool, body: &str| {
        let mut req = Request::builder()
            .uri(format!("/api/v1{path}"))
            .method(method)
            .header("content-type", "application/json")
            .header("origin", "https://panel.example.com");
        if auth {
            req = req.header("cookie", "hproxy_session=session-fixture");
        }
        if csrf {
            req = req.header("x-csrf-token", "csrf-fixture");
        }
        req.body(Body::from(body.to_owned())).unwrap()
    };
    let response = router
        .clone()
        .oneshot(request("/updates", Method::GET, false, false, ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = router
        .clone()
        .oneshot(request("/updates/check", Method::POST, true, false, "{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = router
        .clone()
        .oneshot(request("/updates", Method::GET, true, false, ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(value["source_configured"], false);
    assert_eq!(value["components"].as_array().unwrap().len(), 3);
    assert!(value["components"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["state"] == "unchecked"));
    let response = router
        .clone()
        .oneshot(request("/updates/check", Method::POST, true, true, "{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let value: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(value["error"].as_str().unwrap().contains("未配置"));
    let response = router
        .oneshot(request(
            "/updates/apply",
            Method::POST,
            true,
            true,
            r#"{"build":"../../evil"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
