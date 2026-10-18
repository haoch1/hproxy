use anyhow::{Context, Result};
use hproxy_common::{AddressBindings, Resource, Spec};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeSet;

fn server(c: &Connection, id: &str) -> Result<(String, String)> {
    c.query_row("SELECT ipv4,ipv6 FROM agents WHERE id=?", [id], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })
    .optional()?
    .context("关联的服务器不存在")
}

/// Resolve explicit bindings again when saving: an editor opened before an IP
/// change must never write its stale address snapshot back over the new one.
pub fn normalize(c: &Connection, r: &mut Resource) -> Result<()> {
    normalize_reference(c, r, None)
}

fn normalize_reference(c: &Connection, r: &mut Resource, changed: Option<&str>) -> Result<()> {
    let AddressBindings {
        public_host,
        listen_ip,
        target_agent_id,
    } = &r.address_bindings;
    if (*public_host || *listen_ip) && changed.is_none_or(|id| id == r.agent_id) {
        let (v4, v6) = server(c, &r.agent_id)?;
        if *public_host && (!v4.is_empty() || !v6.is_empty()) {
            r.public_host = if !v4.is_empty() {
                v4.clone()
            } else {
                v6.clone()
            };
        }
        if *listen_ip && (!v4.is_empty() || !v6.is_empty()) {
            r.listen_ip = if v6.is_empty() { "0.0.0.0" } else { "::" }.into();
        }
    }
    if let Some(id) = target_agent_id
        .as_ref()
        .filter(|id| changed.is_none_or(|changed| changed == id.as_str()))
    {
        let (v4, v6) = server(c, id)?;
        if let Spec::Forward {
            remote_host,
            remote_host_v6,
            ..
        } = &mut r.spec
        {
            if !v4.is_empty() || !v6.is_empty() {
                *remote_host = if !v4.is_empty() {
                    v4.clone()
                } else {
                    v6.clone()
                };
                *remote_host_v6 = (!v4.is_empty() && !v6.is_empty()).then_some(v6);
            }
        }
    }
    Ok(())
}

/// One transaction changes the address book and all explicit references. No
/// resource IDs, quota baselines, lifetime counters or reports are replaced.
pub fn report(
    c: &mut Connection,
    agent: &str,
    ipv4: Option<String>,
    ipv6: Option<String>,
) -> Result<bool> {
    let v4 = ipv4
        .map(|s| s.parse::<std::net::Ipv4Addr>().context("IPv4 无效"))
        .transpose()?;
    let v6 = ipv6
        .map(|s| s.parse::<std::net::Ipv6Addr>().context("IPv6 无效"))
        .transpose()?;
    anyhow::ensure!(
        !v4.is_some_and(|ip| ip.is_unspecified() || ip.is_loopback())
            && !v6.is_some_and(|ip| ip.is_unspecified() || ip.is_loopback()),
        "公网地址无效"
    );
    let tx = c.transaction()?;
    let old = server(&tx, agent)?;
    // A failed family probe is unknown, not evidence that the family vanished.
    let next = (
        v4.map(|ip| ip.to_string()).unwrap_or_else(|| old.0.clone()),
        v6.map(|ip| ip.to_string()).unwrap_or_else(|| old.1.clone()),
    );
    if old == next {
        return Ok(false);
    }
    tx.execute(
        "UPDATE agents SET ipv4=?,ipv6=? WHERE id=?",
        params![next.0, next.1, agent],
    )?;
    let mut affected = BTreeSet::new();
    for mut r in crate::db::resources(&tx, None)? {
        if r.agent_id != agent && r.address_bindings.target_agent_id.as_deref() != Some(agent) {
            continue;
        }
        let old = r.clone();
        normalize_reference(&tx, &mut r, Some(agent))?;
        if r == old {
            continue;
        }
        r.validate()?;
        tx.execute(
            "UPDATE resources SET body=? WHERE id=?",
            params![serde_json::to_string(&r)?, r.id],
        )?;
        affected.insert(r.agent_id);
    }
    for id in affected {
        hproxy_common::validate_resources(&crate::db::resources(&tx, Some(&id))?)?;
        crate::db::bump(&tx, &id)?;
    }
    tx.commit()?;
    Ok(true)
}
