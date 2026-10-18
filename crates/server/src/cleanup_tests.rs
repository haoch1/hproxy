use super::core_tests::{fixture, node};
use super::*;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn admin_delete(path: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        .header("cookie", "hproxy_session=fixture")
        .header("x-csrf-token", "csrf")
        .body(Body::empty())
        .unwrap()
}
async fn body(response: Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
fn log(resource: &Resource) -> ConnectionLog {
    ConnectionLog {
        id: resource.id.clone(),
        resource_id: resource.id.clone(),
        network: "tcp".into(),
        source: "private-client:1".into(),
        destination: "private-target:443".into(),
        domain: String::new(),
        started_at: now(),
        closed_at: None,
        upload: 10,
        download: 20,
        result: "active".into(),
    }
}
fn report(resources: &[Resource]) -> Report {
    Report {
        id: id(),
        at: now(),
        counters: resources
            .iter()
            .map(|r| {
                (
                    r.id.clone(),
                    Counter {
                        upload: 10,
                        download: 20,
                    },
                )
            })
            .collect(),
        logs: resources.iter().map(log).collect(),
        cores: vec![],
        dropped_logs: 0, quota_baselines: Default::default(),
    }
}

#[tokio::test]
async fn deleting_node_reuses_desired_revision_and_preserves_controller_history() {
    let (dir, app) = fixture();
    let removed = node();
    let mut kept = node();
    kept.listen_port += 1;
    for r in [&removed, &kept] {
        store_resource(&app, r.clone(), true).unwrap();
    }
    app.ingest_report(&removed.agent_id, &report(&[removed.clone(), kept.clone()]))
        .unwrap();
    let before = *app.command_changes.borrow();
    let response = router(app.clone())
        .oneshot(admin_delete(&format!("/api/v1/resources/{}", removed.id)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body(response).await["revision"].as_i64().unwrap() > 0);
    assert!(*app.command_changes.borrow() > before);
    assert_eq!(
        query_log_page(&app.connection(), &Filter::default()).unwrap()["total"],
        2
    );
    let reopened = db::open(&dir.path().join("test.db")).unwrap();
    assert_eq!(
        db::resources(&reopened, Some(&removed.agent_id)).unwrap(),
        vec![kept.clone()]
    );
    let mut delayed = report(&[removed.clone(), kept.clone()]);
    delayed.logs[0].id = id(); // A late report must not insert a new log for a removed resource.
    app.ingest_report(&removed.agent_id, &delayed).unwrap();
    for table in ["connections", "counters", "traffic"] {
        let c = app.connection();
        let deleted_count: u32 = c
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE resource_id=?"),
                [&removed.id],
                |r| r.get(0),
            )
            .unwrap();
        let retained_count: u32 = c
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE resource_id=?"),
                [&kept.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(deleted_count, 1);
        assert!(retained_count > 0);
    }
    app.connection()
        .execute(
            "UPDATE connections SET started_at=? WHERE resource_id=?",
            params![now() - 8 * 86_400_000, removed.id],
        )
        .unwrap();
    db::cleanup(&app.connection()).unwrap();
    assert_eq!(
        query_log_page(&app.connection(), &Filter::default()).unwrap()["total"],
        1
    );
    assert_eq!(
        app.connection()
            .query_row(
                "SELECT COUNT(*) FROM counters WHERE resource_id=?",
                [&removed.id],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn server_removal_waits_for_an_authenticated_cleanup_receipt_and_is_idempotent() {
    let (_dir, app) = fixture();
    let removed = node();
    store_resource(&app, removed.clone(), true).unwrap();
    app.connection()
        .execute(
            "UPDATE agents SET token_hash=?",
            [digest("removal-fixture")],
        )
        .unwrap();
    app.ingest_report(&removed.agent_id, &report(&[removed.clone()]))
        .unwrap();
    app.connection()
        .execute(
            "INSERT INTO agents(id,name,token_hash,first_seen) VALUES('other','other',?,1)",
            [digest("other-fixture")],
        )
        .unwrap();
    let router = router(app.clone());
    let response = router
        .clone()
        .oneshot(admin_delete(&format!(
            "/api/v1/agents/{}",
            removed.agent_id
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = body(response).await;
    let request_id = result["request_id"].as_str().unwrap();
    assert_eq!(result["cleanup"], "queued");
    let agents = db::list_agents(&app.connection()).unwrap();
    assert!(agents
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"] == removed.agent_id && a["deleting"] == true));
    assert!(!db::resources(&app.connection(), None)
        .unwrap()
        .iter()
        .any(|r| r.agent_id == removed.agent_id));
    app.ingest_report(&removed.agent_id, &report(&[removed.clone()]))
        .unwrap();
    assert_eq!(
        query_log_page(&app.connection(), &Filter::default()).unwrap()["total"],
        1
    );
    let receipt = |token: &str, error: Option<&str>| {
        Request::builder()
            .method("POST")
            .uri("/agent/v1/removal")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"request_id":request_id,"error":error}).to_string(),
            ))
            .unwrap()
    };
    assert_eq!(
        router
            .clone()
            .oneshot(receipt("other-fixture", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router
            .clone()
            .oneshot(receipt("removal-fixture", Some("disk failure")))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(db::list_agents(&app.connection())
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"] == removed.agent_id
            && a["error"].as_str().unwrap().contains("disk failure")));
    assert_eq!(
        router
            .clone()
            .oneshot(receipt("removal-fixture", None))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router
            .oneshot(receipt("removal-fixture", None))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let c = app.connection();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM agents WHERE id=?",
            [&removed.agent_id],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM agents WHERE id='other'", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        1
    );
    for table in ["resources", "jobs"] {
        assert_eq!(
            c.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE agent_id=?"),
                [&removed.agent_id],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
            0
        );
    }
    for table in ["connections", "counters", "traffic", "receipts"] {
        assert!(
            c.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE agent_id=?"),
                [&removed.agent_id],
                |r| r.get::<_, u32>(0)
            )
            .unwrap()
                > 0
        );
    }
    assert_eq!(query_log_page(&c, &Filter::default()).unwrap()["total"], 1);
}

#[tokio::test]
async fn deleting_with_an_old_agent_does_not_falsely_claim_remote_cleanup() {
    let (_dir, app) = fixture();
    let resource = node();
    store_resource(&app, resource.clone(), true).unwrap();
    app.connection()
        .execute(
            "UPDATE agents SET cleanup_supported=0,uninstall_supported=0",
            [],
        )
        .unwrap();
    let router = router(app.clone());
    for path in [
        format!("/api/v1/resources/{}", resource.id),
        format!("/api/v1/agents/{}", resource.agent_id),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(admin_delete(&path))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
    }
    assert_eq!(db::resources(&app.connection(), None).unwrap().len(), 1);
}
