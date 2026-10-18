use anyhow::Result;
use hproxy_common::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::path::Path;
use std::collections::BTreeMap;

pub fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
    CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sessions(hash TEXT PRIMARY KEY,csrf TEXT NOT NULL,expires INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS agents(id TEXT PRIMARY KEY,name TEXT NOT NULL,address TEXT NOT NULL DEFAULT '',token_hash TEXT UNIQUE,revoked INTEGER NOT NULL DEFAULT 0,revision INTEGER NOT NULL DEFAULT 1,applied INTEGER NOT NULL DEFAULT 0,last_seen INTEGER NOT NULL DEFAULT 0,version TEXT NOT NULL DEFAULT '',platform TEXT NOT NULL DEFAULT '',cores TEXT NOT NULL DEFAULT '[]',error TEXT);
    CREATE TABLE IF NOT EXISTS enrollment(hash TEXT PRIMARY KEY,agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,expires INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS resources(id TEXT PRIMARY KEY,agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,body TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS resource_agent ON resources(agent_id);
    CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,revision INTEGER NOT NULL,state TEXT NOT NULL,error TEXT,created_at INTEGER NOT NULL,finished_at INTEGER);
    CREATE INDEX IF NOT EXISTS job_agent ON jobs(agent_id,created_at);
    CREATE TABLE IF NOT EXISTS counters(agent_id TEXT NOT NULL,resource_id TEXT NOT NULL,upload INTEGER NOT NULL,download INTEGER NOT NULL,PRIMARY KEY(agent_id,resource_id));
    CREATE TABLE IF NOT EXISTS traffic(hour INTEGER NOT NULL,agent_id TEXT NOT NULL,resource_id TEXT NOT NULL,upload INTEGER NOT NULL,download INTEGER NOT NULL,PRIMARY KEY(hour,agent_id,resource_id));
    CREATE INDEX IF NOT EXISTS traffic_resource ON traffic(resource_id,hour);
    CREATE TABLE IF NOT EXISTS connections(agent_id TEXT NOT NULL,id TEXT NOT NULL,resource_id TEXT NOT NULL,network TEXT NOT NULL,source TEXT NOT NULL,destination TEXT NOT NULL,domain TEXT NOT NULL,started_at INTEGER NOT NULL,closed_at INTEGER,upload INTEGER NOT NULL,download INTEGER NOT NULL,result TEXT NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(agent_id,id));
    CREATE INDEX IF NOT EXISTS connection_time ON connections(started_at DESC);
    CREATE INDEX IF NOT EXISTS connection_resource ON connections(resource_id,started_at DESC);
    CREATE TABLE IF NOT EXISTS receipts(agent_id TEXT NOT NULL,id TEXT NOT NULL,at INTEGER NOT NULL,PRIMARY KEY(agent_id,id));")?;
    // Preserve databases created before agents reported both address families.
    let columns = c
        .prepare("PRAGMA table_info(agents)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for column in [
        "ipv4",
        "ipv6",
        "country_code",
        "update_build",
        "update_state",
        "update_error",
    ] {
        if !columns.iter().any(|c| c == column) {
            c.execute_batch(&format!(
                "ALTER TABLE agents ADD COLUMN {column} TEXT NOT NULL DEFAULT '';"
            ))?;
        }
    }
    for column in ["update_enabled", "core_management", "component_updates", "cleanup_supported", "uninstall_supported", "quota_cycles"] {
        if !columns.iter().any(|c| c == column) {
            c.execute_batch(&format!(
                "ALTER TABLE agents ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0;"
            ))?;
        }
    }
    if !columns.iter().any(|c| c == "core_policy") {
        c.execute_batch("ALTER TABLE agents ADD COLUMN core_policy TEXT NOT NULL DEFAULT 'null';")?;
    }
    if !columns.iter().any(|c| c == "enabled") {
        c.execute_batch("ALTER TABLE agents ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;")?;
    }
    if !columns.iter().any(|c| c == "first_seen") {
        c.execute_batch("ALTER TABLE agents ADD COLUMN first_seen INTEGER NOT NULL DEFAULT 0;
            UPDATE agents SET first_seen=CASE WHEN last_seen>0 THEN last_seen ELSE 1 END WHERE last_seen>0 OR version!='';")?;
    }
    if !columns.iter().any(|c| c == "deletion_id") {
        c.execute_batch("ALTER TABLE agents ADD COLUMN deletion_id TEXT NOT NULL DEFAULT '';")?;
    }
    c.execute_batch("CREATE TABLE IF NOT EXISTS removal_receipts(hash TEXT NOT NULL,request_id TEXT NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(hash,request_id));")?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS quota_baselines(agent_id TEXT NOT NULL,resource_id TEXT NOT NULL,at INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(agent_id,resource_id));
      CREATE TRIGGER IF NOT EXISTS quota_baseline_delete AFTER DELETE ON resources BEGIN DELETE FROM quota_baselines WHERE resource_id=OLD.id; END;")?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS resource_types(id TEXT PRIMARY KEY,kind TEXT NOT NULL);
      INSERT OR IGNORE INTO resource_types SELECT id,json_extract(body,'$.spec.type') FROM resources;
      DROP TRIGGER IF EXISTS resource_type_insert;
      DROP TRIGGER IF EXISTS resource_type_update;
      CREATE TRIGGER resource_type_insert AFTER INSERT ON resources BEGIN
        INSERT INTO resource_types VALUES(NEW.id,json_extract(NEW.body,'$.spec.type')) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind;
      END;
      CREATE TRIGGER resource_type_update AFTER UPDATE ON resources BEGIN
        INSERT INTO resource_types VALUES(NEW.id,json_extract(NEW.body,'$.spec.type')) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind;
      END;")?;
    c.execute("INSERT OR IGNORE INTO meta VALUES('log_days','7')", [])?;
    c.execute("INSERT OR IGNORE INTO meta VALUES('schema','1')", [])?;
    Ok(c)
}
pub fn meta(c: &Connection, key: &str) -> Result<Option<String>> {
    Ok(
        c.query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))
            .optional()?,
    )
}
pub fn resources(c: &Connection, agent: Option<&str>) -> Result<Vec<Resource>> {
    let mut st =
        c.prepare("SELECT body FROM resources WHERE (?1 IS NULL OR agent_id=?1) ORDER BY rowid")?;
    let bodies = st
        .query_map([agent], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    bodies
        .into_iter()
        .map(|s| Ok(serde_json::from_str(&s)?))
        .collect()
}
pub fn core_policy(c: &Connection, agent: &str) -> Result<Option<Vec<String>>> {
    let body: String = c.query_row("SELECT core_policy FROM agents WHERE id=?", [agent], |r| {
        r.get(0)
    })?;
    Ok(serde_json::from_str(&body)?)
}
pub fn ensure_resource_cores(c: &Connection, agent: &str, resources: &[Resource]) -> Result<()> {
    if let Some(mut cores) = core_policy(c, agent)? {
        for r in resources {
            let core = required_core(r);
            if !cores.iter().any(|c| c == core) {
                cores.push(core.into());
            }
        }
        c.execute(
            "UPDATE agents SET core_policy=? WHERE id=?",
            params![serde_json::to_string(&cores)?, agent],
        )?;
    }
    Ok(())
}
pub fn bump(tx: &Transaction, agent: &str) -> Result<i64> {
    tx.execute(
        "UPDATE agents SET revision=revision+1,error=NULL WHERE id=?",
        [agent],
    )?;
    let rev: i64 = tx.query_row("SELECT revision FROM agents WHERE id=?", [agent], |r| {
        r.get(0)
    })?;
    tx.execute(
        "UPDATE jobs SET state='superseded',finished_at=? WHERE agent_id=? AND state='pending'",
        params![now(), agent],
    )?;
    tx.execute(
        "INSERT INTO jobs(id,agent_id,revision,state,created_at) VALUES(?,?,?,'pending',?)",
        params![id(), agent, rev, now()],
    )?;
    Ok(rev)
}
pub fn ingest(c: &mut Connection, agent: &str, report: &Report) -> Result<bool> {
    if report.logs.len() > 2000 || report.counters.len() > 512 {
        anyhow::bail!("report too large")
    }
    let tx = c.transaction()?;
    let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM agents WHERE id=? AND deletion_id='')", [agent], |row| row.get(0))?;
    if !active { return Ok(false); }
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO receipts VALUES(?,?,?)",
        params![agent, report.id, now()],
    )?;
    if inserted == 0 {
        return Ok(false);
    }
    // Reports may be delayed or replayed. Resource totals only move forward.
    let hour = (report.at.min(now()).max(0) / 3_600_000) * 3_600_000;
    for (resource, total) in &report.counters {
        let owned: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE id=? AND agent_id=?)",
            params![resource, agent],
            |r| r.get(0),
        )?;
        if !owned {
            continue;
        }
        let previous = tx
            .query_row(
                "SELECT upload,download FROM counters WHERE agent_id=? AND resource_id=?",
                params![agent, resource],
                |r| Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?)),
            )
            .optional()?
            .unwrap_or_default();
        let up = total.upload.min(i64::MAX as u64);
        let down = total.download.min(i64::MAX as u64);
        let du = up.saturating_sub(previous.0);
        let dd = down.saturating_sub(previous.1);
        tx.execute("INSERT INTO counters VALUES(?,?,?,?) ON CONFLICT(agent_id,resource_id) DO UPDATE SET upload=MAX(upload,excluded.upload),download=MAX(download,excluded.download)",params![agent,resource,up,down])?;
        if du + dd > 0 {
            tx.execute("INSERT INTO traffic VALUES(?,?,?,?,?) ON CONFLICT(hour,agent_id,resource_id) DO UPDATE SET upload=upload+excluded.upload,download=download+excluded.download",params![hour,agent,resource,du,dd])?;
        }
        if let Some(baseline) = report.quota_baselines.get(resource) {
            if baseline.cycle.valid() && baseline.start <= report.at
                && baseline.counter.upload <= total.upload && baseline.counter.download <= total.download {
                tx.execute("INSERT INTO quota_baselines VALUES(?,?,?,?) ON CONFLICT(agent_id,resource_id) DO UPDATE SET at=excluded.at,body=excluded.body WHERE excluded.at>=quota_baselines.at",
                    params![agent,resource,report.at,serde_json::to_string(baseline)?])?;
            }
        }
    }
    let retention = meta(&tx, "log_days")?
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(7);
    let cutoff = now() - retention * 86_400_000;
    let mut logs_changed = false;
    for log in &report.logs {
        if log.started_at < cutoff
            || log.source.len() > 512
            || log.destination.len() > 512
            || log.domain.len() > 253
            || log.result.len() > 2048
        {
            continue;
        }
        let owned: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE id=? AND agent_id=? AND json_extract(body,'$.spec.type') IN ('vless','shadowsocks'))",
            params![log.resource_id, agent],
            |r| r.get(0),
        )?;
        if !owned {
            continue;
        }
        tx.execute("INSERT INTO connections VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(agent_id,id) DO UPDATE SET closed_at=COALESCE(excluded.closed_at,closed_at),upload=MAX(upload,excluded.upload),download=MAX(download,excluded.download),domain=CASE WHEN excluded.domain='' THEN domain ELSE excluded.domain END,result=excluded.result,updated_at=excluded.updated_at",params![agent,log.id,log.resource_id,log.network,log.source,log.destination,log.domain,log.started_at,log.closed_at,log.upload.min(i64::MAX as u64),log.download.min(i64::MAX as u64),log.result,now()])?;
        logs_changed = true;
    }
    tx.execute(
        "UPDATE agents SET cores=?,last_seen=? WHERE id=?",
        params![serde_json::to_string(&report.cores)?, now(), agent],
    )?;
    tx.commit()?;
    Ok(logs_changed)
}
pub fn resource_counters(c: &Connection, resources: &[Resource]) -> Result<BTreeMap<String, Counter>> {
    let mut counters = c.prepare("SELECT resource_id,upload,download FROM counters")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,Counter { upload:r.get(1)?, download:r.get(2)? })))?
        .collect::<rusqlite::Result<BTreeMap<_,_>>>()?;
    let baselines = c.prepare("SELECT resource_id,body FROM quota_baselines")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
        .collect::<rusqlite::Result<BTreeMap<_,_>>>()?;
    for resource in resources {
        if let Some(cycle) = &resource.quota_cycle {
            if let Some(body) = baselines.get(&resource.id) {
                let baseline: QuotaBaseline = serde_json::from_str(body)?;
                let total = counters.entry(resource.id.clone()).or_default();
                *total = if &baseline.cycle == cycle && cycle.window(now()).is_some_and(|(start,_)|start>baseline.start) {
                    Counter::default()
                } else { baseline.usage(total) };
            }
        }
    }
    Ok(counters)
}
pub fn list_agents(c: &Connection) -> Result<Value> {
    let mut s=c.prepare("SELECT id,name,COALESCE(NULLIF(address,''),NULLIF(ipv4,''),ipv6),revoked,revision,applied,last_seen,version,platform,cores,error,token_hash IS NOT NULL,ipv4,ipv6,country_code,update_enabled,update_build,update_state,update_error,core_policy,core_management,enabled FROM agents WHERE first_seen>0 ORDER BY rowid")?;
    let v=s.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"address":r.get::<_,String>(2)?,"revoked":r.get::<_,bool>(3)?,"revision":r.get::<_,i64>(4)?,"applied":r.get::<_,i64>(5)?,"last_seen":r.get::<_,i64>(6)?,"version":r.get::<_,String>(7)?,"platform":r.get::<_,String>(8)?,"cores":serde_json::from_str::<Value>(&r.get::<_,String>(9)?).unwrap_or(json!([])),"error":r.get::<_,Option<String>>(10)?,"enrolled":r.get::<_,bool>(11)?,"ipv4":r.get::<_,String>(12)?,"ipv6":r.get::<_,String>(13)?,"country_code":r.get::<_,String>(14)?,"update_enabled":r.get::<_,bool>(15)?,"update_build":r.get::<_,String>(16)?,"update_state":r.get::<_,String>(17)?,"update_error":r.get::<_,String>(18)?,"core_policy":serde_json::from_str::<Value>(&r.get::<_,String>(19)?).unwrap_or(Value::Null),"core_management":r.get::<_,bool>(20)?,"enabled":r.get::<_,bool>(21)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let cleanup = c.prepare("SELECT id,deletion_id,cleanup_supported,uninstall_supported,quota_cycles FROM agents")?
        .query_map([], |row| Ok((row.get::<_,String>(0)?, json!({"deleting": !row.get::<_,String>(1)?.is_empty(),"cleanup_supported":row.get::<_,bool>(2)?,"uninstall_supported":row.get::<_,bool>(3)?,"quota_cycles":row.get::<_,bool>(4)?}))))?
        .collect::<rusqlite::Result<std::collections::BTreeMap<_,_>>>()?;
    let mut values = json!(v);
    for row in values.as_array_mut().unwrap() {
        if let Some(fields) = row["id"].as_str().and_then(|id| cleanup.get(id)) {
            for (key, value) in fields.as_object().unwrap() { row[key] = value.clone(); }
        }
    }
    Ok(values)
}
pub fn cleanup(c: &Connection) -> Result<()> {
    let days = meta(c, "log_days")?
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(7);
    c.execute(
        "DELETE FROM connections WHERE started_at<? OR resource_id IN (SELECT id FROM resource_types WHERE kind='forward')",
        [now() - days * 86_400_000],
    )?;
    c.execute("DELETE FROM sessions WHERE expires<?", [now()])?;
    c.execute("DELETE FROM removal_receipts WHERE expires<?", [now()])?;
    c.execute("DELETE FROM enrollment WHERE expires<?", [now()])?;
    c.execute("DELETE FROM receipts WHERE at<?", [now() - 32 * 86_400_000])?;
    c.execute(
        "DELETE FROM jobs WHERE created_at<? AND state!='pending'",
        [now() - 30 * 86_400_000],
    )?;
    c.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn ingest_rejects_forward_and_expired_logs_but_keeps_traffic_counters() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = open(&dir.path().join("db")).unwrap();
        c.execute("INSERT INTO agents(id,name) VALUES('agent','test')", []).unwrap();
        c.execute_batch("INSERT INTO resources VALUES('node','agent','{\"spec\":{\"type\":\"vless\"}}'),('forward','agent','{\"spec\":{\"type\":\"forward\"}}');").unwrap();
        let log = |id: &str, resource: &str, started_at| ConnectionLog { id:id.into(), resource_id:resource.into(), network:"tcp".into(), source:"client".into(), destination:"target".into(), domain:String::new(), started_at, closed_at:Some(now()), upload:10, download:20, result:"completed".into() };
        let report = Report { id:id(), at:now(), counters:BTreeMap::from([("forward".into(), Counter {upload:10,download:20})]), logs:vec![log("node", "node", now()),log("forward", "forward", now()),log("expired", "node", now()-8*86_400_000)], cores:vec![], dropped_logs:0, quota_baselines: Default::default() };
        ingest(&mut c, "agent", &report).unwrap();
        assert_eq!(c.query_row("SELECT COUNT(*) FROM connections", [], |r|r.get::<_,u32>(0)).unwrap(), 1);
        assert_eq!(c.query_row("SELECT SUM(upload+download) FROM counters", [], |r|r.get::<_,u64>(0)).unwrap(), 30);
        assert_eq!(c.query_row("SELECT SUM(upload+download) FROM traffic", [], |r|r.get::<_,u64>(0)).unwrap(), 30);
    }
    #[test]
    fn replay_does_not_double_bill_and_cross_agent_logs_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = open(&dir.path().join("db")).unwrap();
        let agent = id();
        let resource = id();
        c.execute("INSERT INTO agents(id,name) VALUES(?,'test')", [&agent])
            .unwrap();
        c.execute(
            "INSERT INTO resources VALUES(?,?, '{\"spec\":{\"type\":\"vless\"}}')",
            params![resource, agent],
        )
        .unwrap();
        let mut report = Report {
            id: id(),
            at: now(),
            counters: Default::default(),
            logs: vec![],
            cores: vec![],
            dropped_logs: 0, quota_baselines: Default::default(),
        };
        report.counters.insert(
            resource.clone(),
            Counter {
                upload: 100,
                download: 200,
            },
        );
        ingest(&mut c, &agent, &report).unwrap();
        ingest(&mut c, &agent, &report).unwrap();
        report.id = id();
        report.counters.get_mut(&resource).unwrap().upload = 120;
        ingest(&mut c, &agent, &report).unwrap();
        let total: u64 = c
            .query_row("SELECT SUM(upload+download) FROM traffic", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 320);
        report.id = id();
        report.counters.get_mut(&resource).unwrap().upload = 50;
        ingest(&mut c, &agent, &report).unwrap();
        let total: u64 = c
            .query_row("SELECT SUM(upload+download) FROM traffic", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 320);
    }
}
