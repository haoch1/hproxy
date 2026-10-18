use super::*;
use hproxy_common::AddressBindings;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const RELAY: &str = "00000000-0000-4000-8000-000000000002";

fn revision(app: &App, id: &str) -> i64 {
    app.connection()
        .query_row("SELECT revision FROM agents WHERE id=?", [id], |r| r.get(0))
        .unwrap()
}
fn get(app: &App, id: &str) -> Resource {
    db::resources(&app.connection(), None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap()
}
fn history(app: &App) -> Vec<Vec<String>> {
    let c = app.connection();
    [
        ("counters", "agent_id,resource_id,upload,download"),
        ("traffic", "hour,agent_id,resource_id,upload,download"),
        ("quota_baselines", "agent_id,resource_id,at,body"),
        ("receipts", "agent_id,id,at"),
    ]
    .into_iter()
    .map(|(table, columns)| {
        let value = columns
            .split(',')
            .map(|column| format!("quote({column})"))
            .collect::<Vec<_>>()
            .join("||':'||");
        c.prepare(&format!("SELECT {value} FROM {table} ORDER BY 1"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap()
    })
    .collect()
}

#[test]
fn address_reports_sync_both_roles_once_and_preserve_history_and_manual_addresses() {
    let (_dir, app) = core_tests::fixture();
    app.connection().execute("INSERT INTO agents(id,name,ipv4,core_policy,core_management,quota_cycles) VALUES(?,'relay','192.0.2.2','[]',1,1)", [RELAY]).unwrap();
    app.connection()
        .execute(
            "UPDATE agents SET ipv4='192.0.2.1',quota_cycles=1 WHERE id=?",
            [OWNER],
        )
        .unwrap();
    let mut node = core_tests::node();
    node.quota_bytes = Some(1024);
    node.quota_cycle = Some(QuotaCycle {
        starts_at: now() - 60_000,
        months: 1,
        days: 0,
    });
    node.address_bindings = AddressBindings {
        public_host: true,
        listen_ip: true,
        target_agent_id: None,
    };
    store_resource(&app, node.clone(), true).unwrap();
    let mut forward = core_tests::node();
    forward.agent_id = RELAY.into();
    forward.spec = Spec::Forward {
        remote_host: "192.0.2.1".into(),
        remote_host_v6: None,
        remote_port: 443,
        network: "tcp_udp".into(),
    };
    forward.address_bindings = AddressBindings {
        public_host: true,
        listen_ip: true,
        target_agent_id: Some(OWNER.into()),
    };
    store_resource(&app, forward.clone(), true).unwrap();
    let mut manual = forward.clone();
    manual.id = id();
    manual.listen_port = 9443;
    manual.public_host = "custom.example.com".into();
    manual.address_bindings = Default::default();
    store_resource(&app, manual.clone(), true).unwrap();
    let mut manual_node = node.clone();
    manual_node.id = id();
    manual_node.listen_port = 9443;
    manual_node.address_bindings = Default::default();
    store_resource(&app, manual_node.clone(), true).unwrap();
    let baseline = QuotaBaseline {
        cycle: node.quota_cycle.clone().unwrap(),
        start: node.quota_cycle.as_ref().unwrap().window(now()).unwrap().0,
        counter: Counter::default(),
    };
    app.ingest_report(
        OWNER,
        &Report {
            id: id(),
            at: now(),
            counters: std::collections::BTreeMap::from([(
                node.id.clone(),
                Counter {
                    upload: 123,
                    download: 456,
                },
            )]),
            quota_baselines: std::collections::BTreeMap::from([(node.id.clone(), baseline)]),
            logs: vec![],
            cores: vec![],
            dropped_logs: 0,
        },
    )
    .unwrap();
    let history_before = history(&app);
    let revisions = (revision(&app, OWNER), revision(&app, RELAY));
    assert!(crate::addresses::report(
        &mut app.connection(),
        OWNER,
        Some("198.51.100.10".into()),
        Some("2001:db8::10".into())
    )
    .unwrap());
    let changed = get(&app, &node.id);
    assert_eq!(changed.public_host, "198.51.100.10");
    assert_eq!(changed.listen_ip, "::");
    assert_eq!(changed.quota_cycle, node.quota_cycle);
    assert_eq!(changed.quota_bytes, node.quota_bytes);
    let landing = get(&app, &forward.id);
    assert_eq!(landing.agent_id, RELAY);
    assert!(
        matches!(&landing.spec, Spec::Forward { remote_host, remote_host_v6:Some(v6), .. } if remote_host=="198.51.100.10" && v6=="2001:db8::10")
    );
    assert_eq!(get(&app, &manual.id), manual);
    assert_eq!(get(&app, &manual_node.id), manual_node);
    assert_eq!(
        (revision(&app, OWNER), revision(&app, RELAY)),
        (revisions.0 + 1, revisions.1 + 1)
    );
    assert_eq!(history(&app), history_before);
    assert!(!crate::addresses::report(
        &mut app.connection(),
        OWNER,
        Some("198.51.100.10".into()),
        None
    )
    .unwrap());
    assert_eq!(revision(&app, OWNER), revisions.0 + 1);
    // A long-offline relay receives the same updated resource ID on reconnect.
    let persisted = get(&app, &forward.id);
    assert!(desired_resources(&app.connection(), RELAY)
        .unwrap()
        .iter()
        .any(|r| r.id == forward.id && r.spec == persisted.spec));
    // The relay's own IP changes independently of its destination binding.
    crate::addresses::report(
        &mut app.connection(),
        RELAY,
        Some("198.51.100.20".into()),
        None,
    )
    .unwrap();
    assert_eq!(get(&app, &forward.id).public_host, "198.51.100.20");
    assert_eq!(get(&app, &manual.id), manual);
    assert_eq!(history(&app), history_before);
    // Old editor snapshots are normalized at save time, without resetting usage.
    store_resource(&app, node.clone(), false).unwrap();
    store_resource(&app, forward.clone(), false).unwrap();
    assert_eq!(get(&app, &node.id).public_host, "198.51.100.10");
    assert_eq!(get(&app, &forward.id).spec, landing.spec);
    assert_eq!(history(&app), history_before);
}

#[test]
fn invalid_reports_and_legacy_records_do_not_change_saved_addresses() {
    let (_dir, app) = core_tests::fixture();
    let mut json = serde_json::to_value(core_tests::node()).unwrap();
    json.as_object_mut().unwrap().remove("address_bindings");
    let manual: Resource = serde_json::from_value(json).unwrap();
    store_resource(&app, manual.clone(), true).unwrap();
    let before = revision(&app, OWNER);
    assert!(crate::addresses::report(
        &mut app.connection(),
        OWNER,
        Some("198.51.100.2".into()),
        Some("bad ipv6".into())
    )
    .is_err());
    assert_eq!(get(&app, &manual.id), manual);
    assert_eq!(revision(&app, OWNER), before);
    crate::addresses::report(
        &mut app.connection(),
        OWNER,
        None,
        Some("2001:db8::2".into()),
    )
    .unwrap();
    assert_eq!(get(&app, &manual.id), manual);
    assert_eq!(revision(&app, OWNER), before);
}
