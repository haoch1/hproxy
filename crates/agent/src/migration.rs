use anyhow::{Context, Result};
use hproxy_common::*;
use serde_json::Value;
use std::path::PathBuf;
pub fn legacy_import(
    agent: &str,
    sing: Option<PathBuf>,
    nodes: Option<PathBuf>,
    realm: Option<PathBuf>,
    output: PathBuf,
) -> Result<()> {
    let mut resources = vec![];
    let metadata: Value = nodes
        .map(|p| std::fs::read(p).map(|b| serde_json::from_slice(&b)))
        .transpose()?
        .transpose()?
        .unwrap_or(Value::Null);
    if let Some(file) = sing {
        let config: Value = serde_json::from_slice(&std::fs::read(file)?)?;
        for inbound in config["inbounds"].as_array().context("配置缺少 inbounds")? {
            let tag = inbound["tag"].as_str().unwrap_or("");
            let meta = metadata
                .as_array()
                .and_then(|items| items.iter().find(|v| v["tag"].as_str() == Some(tag)));
            let spec = match inbound["type"].as_str() {
                Some("vless") => {
                    let p = inbound["tls"]["reality"]["private_key"]
                        .as_str()
                        .context("Reality 私钥缺失")?;
                    let mut spec = Resource::generate("vless")?;
                    if let Spec::Vless {
                        uuid,
                        private_key,
                        public_key,
                        short_id,
                        sni,
                    } = &mut spec
                    {
                        use base64::Engine;
                        let bytes: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .decode(p)?
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("私钥长度错误"))?;
                        *private_key = p.into();
                        *public_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                            x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(bytes))
                                .as_bytes(),
                        );
                        *uuid = inbound["users"][0]["uuid"]
                            .as_str()
                            .context("UUID 缺失")?
                            .into();
                        *short_id = inbound["tls"]["reality"]["short_id"][0]
                            .as_str()
                            .context("short_id 缺失")?
                            .into();
                        *sni = inbound["tls"]["server_name"]
                            .as_str()
                            .unwrap_or("www.bing.com")
                            .into();
                    }
                    spec
                }
                Some("shadowsocks") if inbound["method"] == "2022-blake3-aes-128-gcm" => {
                    Spec::Shadowsocks {
                        password: inbound["password"].as_str().context("SS 密钥缺失")?.into(),
                    }
                }
                _ => anyhow::bail!("导入包含不支持的入站协议：{tag}"),
            };
            let port = u16::try_from(inbound["listen_port"].as_u64().context("端口缺失")?)?;
            resources.push(Resource {
                id: id(),
                agent_id: agent.into(),
                name: meta.and_then(|v| v["name"].as_str()).unwrap_or(tag).into(),
                enabled: true,
                listen_ip: inbound["listen"].as_str().unwrap_or("::").into(),
                listen_port: port,
                address_bindings: Default::default(), public_host: meta
                    .and_then(|v| v["server"].as_str())
                    .unwrap_or("CHANGE-ME.example")
                    .into(),
                quota_bytes: None, quota_cycle: None,
                expires_at: None,
                spec,
            });
        }
    }
    if let Some(file) = realm {
        let config: Value = serde_json::from_slice(&std::fs::read(file)?)?;
        let mut forwards = Vec::new();
        for ep in config["endpoints"]
            .as_array()
            .context("配置缺少 endpoints")?
        {
            let listen: std::net::SocketAddr =
                ep["listen"].as_str().context("监听地址缺失")?.parse()?;
            let remote = ep["remote"].as_str().context("远端地址缺失")?;
            let (host, port) = remote.rsplit_once(':').context("远端端口缺失")?;
            let no_tcp = ep["network"]["no_tcp"]
                .as_bool()
                .or(config["network"]["no_tcp"].as_bool())
                .unwrap_or(false);
            let udp = ep["network"]["use_udp"]
                .as_bool()
                .or(config["network"]["use_udp"].as_bool())
                .unwrap_or(false);
            let resource = Resource {
                id: id(),
                agent_id: agent.into(),
                name: format!("转发 {}", listen.port()),
                enabled: true,
                listen_ip: listen.ip().to_string(),
                listen_port: listen.port(),
                address_bindings: Default::default(), public_host: "CHANGE-ME.example".into(),
                quota_bytes: None, quota_cycle: None,
                expires_at: None,
                spec: Spec::Forward {
                    remote_host_v6: None,
                    remote_host: host.trim_matches(['[', ']']).into(),
                    remote_port: port.parse()?,
                    network: if no_tcp {
                        "udp"
                    } else if udp {
                        "tcp_udp"
                    } else {
                        "tcp"
                    }
                    .into(),
                },
            };
            let ipv6_only = ep["network"]["ipv6_only"]
                .as_bool()
                .or(config["network"]["ipv6_only"].as_bool())
                .unwrap_or(false);
            forwards.push((resource, ipv6_only));
        }
        resources.extend(merge_realm_pairs(forwards));
    }
    validate_resources(&resources)?;
    anyhow::ensure!(!output.exists(), "输出文件已存在");
    atomic_json(&output, &resources)?;
    println!(
        "已生成 {} 条导入预览。请检查访问地址与端口后在面板导入；原脚本未修改。",
        resources.len()
    );
    Ok(())
}

// A family-preserving dual-stack forward is represented by two native Realm
// endpoints. Merge only complementary wildcard listeners with matching ports,
// protocols and literal destination families; unrelated collisions still fail.
fn merge_realm_pairs(forwards: Vec<(Resource, bool)>) -> Vec<Resource> {
    let mut merged: Vec<(Resource, bool)> = Vec::new();
    for (candidate, v6_only) in forwards {
        let pair = merged.iter_mut().find(|(saved, saved_v6_only)| {
            let (v4, v6, exclusive) = if saved.listen_ip == "0.0.0.0" && candidate.listen_ip == "::"
            {
                (&*saved, &candidate, v6_only)
            } else if saved.listen_ip == "::" && candidate.listen_ip == "0.0.0.0" {
                (&candidate, &*saved, *saved_v6_only)
            } else {
                return false;
            };
            match (&v4.spec, &v6.spec) {
                (
                    Spec::Forward {
                        remote_host: a,
                        remote_host_v6: None,
                        remote_port: ap,
                        network: an,
                    },
                    Spec::Forward {
                        remote_host: b,
                        remote_host_v6: None,
                        remote_port: bp,
                        network: bn,
                    },
                ) => {
                    exclusive
                        && v4.listen_port == v6.listen_port
                        && ap == bp
                        && an == bn
                        && a.parse::<std::net::Ipv4Addr>().is_ok()
                        && b.parse::<std::net::Ipv6Addr>().is_ok()
                }
                _ => false,
            }
        });
        if let Some((saved, _)) = pair {
            let (v4, v6) = if saved.listen_ip == "0.0.0.0" {
                (&saved.spec, &candidate.spec)
            } else {
                (&candidate.spec, &saved.spec)
            };
            if let (
                Spec::Forward {
                    remote_host,
                    remote_port,
                    network,
                    ..
                },
                Spec::Forward {
                    remote_host: v6, ..
                },
            ) = (v4, v6)
            {
                saved.spec = Spec::Forward {
                    remote_host: remote_host.clone(),
                    remote_host_v6: Some(v6.clone()),
                    remote_port: *remote_port,
                    network: network.clone(),
                };
                saved.listen_ip = "::".into();
            }
        } else {
            merged.push((candidate, v6_only));
        }
    }
    merged.into_iter().map(|(r, _)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn own_dual_stack_realm_config_round_trips_in_either_order() {
        for network in ["tcp", "udp", "tcp_udp"] {
            let agent = id();
            let original = Resource {
                id: id(),
                agent_id: agent.clone(),
                name: "dual stack".into(),
                enabled: true,
                listen_ip: "::".into(),
                listen_port: 8443,
                address_bindings: Default::default(), public_host: "example.com".into(),
                quota_bytes: None, quota_cycle: None,
                expires_at: None,
                spec: Spec::Forward {
                    remote_host: "192.0.2.1".into(),
                    remote_host_v6: Some("2001:db8::1".into()),
                    remote_port: 8443,
                    network: network.into(),
                },
            };
            let mut config = crate::cores::realm_config(&[original.clone()]);
            for reverse in [false, true] {
                if reverse {
                    config["endpoints"].as_array_mut().unwrap().reverse();
                }
                let dir = std::env::temp_dir().join(format!("hproxy-import-{}", id()));
                std::fs::create_dir_all(&dir).unwrap();
                let input = dir.join("realm.json");
                let output = dir.join("preview.json");
                std::fs::write(&input, serde_json::to_vec(&config).unwrap()).unwrap();
                legacy_import(&agent, None, None, Some(input), output.clone()).unwrap();
                let imported: Vec<Resource> =
                    serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
                assert_eq!(imported.len(), 1);
                assert_eq!(
                    serde_json::to_value(&imported[0].spec).unwrap(),
                    serde_json::to_value(&original.spec).unwrap()
                );
                assert_eq!(imported[0].listen_ip, "::");
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }
}
