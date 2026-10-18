import type { Agent, Resource, Counter } from "./types";
import { address } from "./types";

// Example output only; production reads the actual core-generated file from the agent.
export function demoConfig(core: string, agent: Agent, resources: Resource[], counters: Record<string, Counter>) {
  if (!["sing-box", "realm"].includes(core)) throw new Error("核心名称无效");
  const selected = resources.filter((r) => {
    const count = counters[r.id];
    return r.agent_id === agent.id && agent.enabled !== false && r.enabled &&
      (!r.expires_at || r.expires_at > Date.now()) &&
      (!r.quota_bytes || (count?.upload || 0) + (count?.download || 0) < r.quota_bytes);
  });
  const content = core === "sing-box" ? {
    log: { level: "warn", timestamp: true },
    inbounds: selected.filter((r) => r.spec.type !== "forward").map((r) => {
      const base = { type: r.spec.type, tag: r.id, listen: r.listen_ip, listen_port: r.listen_port };
      return r.spec.type === "vless" ? { ...base,
        users: [{ name: r.id, uuid: r.spec.uuid, flow: "xtls-rprx-vision" }],
        tls: { enabled: true, server_name: r.spec.sni, reality: { enabled: true,
          handshake: { server: r.spec.sni, server_port: 443 }, private_key: r.spec.private_key, short_id: [r.spec.short_id] } },
      } : r.spec.type === "shadowsocks" ? { ...base, method: "2022-blake3-aes-128-gcm", password: r.spec.password } : base;
    }),
    outbounds: [{ type: "direct", tag: "direct" }],
    route: { rules: [{ action: "sniff", sniffer: ["http", "tls", "quic"], timeout: "300ms" }], final: "direct" },
    services: [{ type: "api", listen: "127.0.0.1", listen_port: 9090, secret: "demo-only-api-secret", dashboard: false }],
  } : {
    log: { level: "warn" }, dns: { mode: "ipv4_and_ipv6", min_ttl: 0, max_ttl: 30, negative_max_ttl: 5 }, network: { tcp_timeout: 10, udp_timeout: 30 },
    endpoints: selected.flatMap((r) => {
      if (r.spec.type !== "forward") return [];
      const spec = r.spec;
      const endpoint = (listen: string, remote: string, ipv6_only: boolean) => ({
        listen: address(listen, r.listen_port), remote: address(remote, spec.remote_port),
        network: { no_tcp: spec.network === "udp", use_udp: spec.network !== "tcp", ipv6_only },
      });
      return r.listen_ip === "::" && spec.remote_host_v6
        ? [endpoint("0.0.0.0", spec.remote_host, false), endpoint("::", spec.remote_host_v6, true)]
        : [endpoint(r.listen_ip, spec.remote_host, false)];
    }),
  };
  return { filename: `${core}.json`, content };
}
