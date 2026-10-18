use super::{query_log_page, query_logs, query_month_traffic, Filter};
use crate::db;
use rusqlite::{params, Connection};

#[test]
fn pagination_counts_filtered_logs_and_clamps_missing_pages() {
    let (_dir, c) = fixture();
    for i in 0..102 {
        c.execute("INSERT INTO connections SELECT agent_id,?1,resource_id,network,source,destination,domain,?2,closed_at,upload,download,result,updated_at FROM connections WHERE id='a'", params![format!("extra-{i:03}"), 5000 + i]).unwrap();
    }
    let all = query_logs(&c, &Filter::default(), 1000, 0).unwrap();
    for size in [20, 50, 100] {
        let mut filter = Filter {
            page_size: Some(size),
            ..Default::default()
        };
        let first = query_log_page(&c, &filter).unwrap();
        assert_eq!(first["total"], 105);
        assert_eq!(first["pages"], 105_u32.div_ceil(size));
        assert_eq!(first["rows"].as_array().unwrap().len(), size as usize);
        filter.page = Some(2);
        let second = query_log_page(&c, &filter).unwrap();
        assert_eq!(second["rows"][0], all[size as usize]);
        filter.page = Some(999);
        let last = query_log_page(&c, &filter).unwrap();
        assert_eq!(last["page"], last["pages"]);
        assert_eq!(last["has_more"], false);
    }
    let filtered = query_log_page(
        &c,
        &Filter {
            status: Some("failed".into()),
            page: Some(8),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(filtered["total"], 1);
    assert_eq!(filtered["page"], 1);
    assert_eq!(filtered["rows"][0]["id"], "c");
    let empty = query_log_page(
        &c,
        &Filter {
            q: Some("no-match".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["pages"], 1);
    assert!(query_log_page(
        &c,
        &Filter {
            page_size: Some(500),
            ..Default::default()
        }
    )
    .is_err());
}

#[test]
fn month_traffic_uses_beijing_calendar_and_same_scope_for_all_servers() {
    let (_dir, c) = fixture();
    let millis = |date: &str| {
        chrono::DateTime::parse_from_rfc3339(date)
            .unwrap()
            .timestamp_millis()
    };
    let start = millis("2026-10-01T00:00:00+08:00");
    for (at, agent, resource, upload, download) in [
        (start - 3_600_000, "server-a", "node", 900, 900),
        (start, "server-a", "node", 10, 20),
        (start + 3_600_000, "server-b", "node", 30, 40),
        (start, "server-a", "forward", 500, 500),
        (
            millis("2026-11-01T00:00:00+08:00"),
            "server-a",
            "node",
            900,
            900,
        ),
    ] {
        c.execute(
            "INSERT INTO traffic VALUES(?,?,?,?,?)",
            params![at, agent, resource, upload, download],
        )
        .unwrap();
    }
    let totals = query_month_traffic(&c, millis("2026-10-31T23:59:59+08:00")).unwrap();
    assert_eq!(totals.get("server-a"), Some(&30));
    assert_eq!(totals.get("server-b"), Some(&70));
    assert_eq!(totals.values().sum::<u64>(), 100);
    let first_hour = query_month_traffic(&c, start).unwrap();
    assert_eq!(first_hour.get("server-a"), Some(&30));
    assert!(!first_hour.contains_key("server-b"));
}

fn fixture() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let c = db::open(&dir.path().join("logs.db")).unwrap();
    c.execute_batch(
        "INSERT INTO resource_types VALUES('node','shadowsocks'),('forward','forward');",
    )
    .unwrap();
    for (id, agent, resource, network, started, closed, result) in [
        ("a", "server-a", "node", "tcp", 1000, None, "active"),
        (
            "b",
            "server-a",
            "node",
            "udp",
            2000,
            Some(2200),
            "completed",
        ),
        (
            "c",
            "server-b",
            "node",
            "tcp",
            3000,
            Some(3100),
            "error: timeout",
        ),
        (
            "d",
            "server-a",
            "forward",
            "tcp",
            2000,
            Some(2500),
            "completed",
        ),
    ] {
        c.execute("INSERT INTO connections VALUES(?,?,?,?,?,'[2001:db8::1]:443','example.com',?,?,10,20,?,4000)", params![agent,id,resource,network,"203.0.113.23:41000",started,closed,result]).unwrap();
    }
    (dir, c)
}

#[test]
fn combines_time_server_source_target_protocol_and_status() {
    let (_dir, c) = fixture();
    let filter = Filter {
        agent_id: Some("server-a".into()),
        from: Some(2000),
        to: Some(2000),
        source: Some("203.0.113.23".into()),
        target: Some("EXAMPLE.COM:443".into()),
        network: Some("udp".into()),
        status: Some("closed".into()),
        ..Default::default()
    };
    let rows = query_logs(&c, &filter, 100, 0).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "b");
    assert!(query_logs(
        &c,
        &Filter {
            network: Some("tcp".into()),
            ..filter
        },
        100,
        0
    )
    .unwrap()
    .as_array()
    .unwrap()
    .is_empty());
}

#[test]
fn keeps_active_closed_failed_disjoint_and_ignores_forwards() {
    let (_dir, c) = fixture();
    for (status, id) in [("active", "a"), ("closed", "b"), ("failed", "c")] {
        let rows = query_logs(
            &c,
            &Filter {
                status: Some(status.into()),
                ..Default::default()
            },
            100,
            0,
        )
        .unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], id);
    }
    let all = query_logs(&c, &Filter::default(), 10000, 0).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 3);
    assert_eq!(query_logs(&c, &Filter::default(), 1, 1).unwrap()[0], all[1]);
}

#[test]
fn treats_search_as_literal_and_rejects_invalid_conditions() {
    let (_dir, c) = fixture();
    for q in ["%", "_", "' OR 1=1 --"] {
        assert!(query_logs(
            &c,
            &Filter {
                q: Some(q.into()),
                ..Default::default()
            },
            100,
            0
        )
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    }
    assert!(query_logs(
        &c,
        &Filter {
            from: Some(3000),
            to: Some(1000),
            ..Default::default()
        },
        100,
        0
    )
    .is_err());
    assert!(query_logs(
        &c,
        &Filter {
            network: Some("invalid".into()),
            ..Default::default()
        },
        100,
        0
    )
    .is_err());
    assert!(query_logs(
        &c,
        &Filter {
            status: Some("invalid".into()),
            ..Default::default()
        },
        100,
        0
    )
    .is_err());
}
