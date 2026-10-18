export type Spec =
  | {
      type: "vless";
      uuid: string;
      private_key: string;
      public_key: string;
      short_id: string;
      sni: string;
    }
  | { type: "shadowsocks"; password: string }
  | {
      type: "forward";
      remote_host: string;
      remote_host_v6?: string | null;
      remote_port: number;
      network: "tcp" | "udp" | "tcp_udp";
    };
export interface Resource {
  id: string;
  agent_id: string;
  name: string;
  enabled: boolean;
  listen_ip: string;
  listen_port: number;
  public_host: string;
  address_bindings?: { public_host: boolean; listen_ip: boolean; target_agent_id?: string | null };
  quota_bytes: number | null;
  quota_cycle?: QuotaCycle | null;
  expires_at: number | null;
  spec: Spec;
}
export interface QuotaCycle { starts_at: number; months: number; days: number }
export interface QuotaBaseline { cycle: QuotaCycle; start: number; counter: Counter }
export interface Counter {
  upload: number;
  download: number;
}
export interface Core {
  core: string;
  state: string;
  version: string;
  error: string | null;
}
export interface Agent {
  id: string;
  name: string;
  address: string;
  ipv4?: string;
  ipv6?: string;
  country_code?: string;
  revoked: boolean;
  enabled?: boolean;
  revision: number;
  applied: number;
  last_seen: number;
  version: string;
  platform: string;
  cores: Core[];
  core_policy?: string[] | null;
  core_management?: boolean;
  quota_cycles?: boolean;
  cleanup_supported?: boolean;
  uninstall_supported?: boolean;
  deleting?: boolean;
  error: string | null;
  enrolled: boolean;
}
export const statusLabels = {
  enabled: "运行中", stopped: "已停止", offline: "离线", pending: "处理中", failed: "失败",
} as const;
export interface DisplayStatus {
  kind: keyof typeof statusLabels;
  reason?: string;
}
export function applicationIssue(agent?: Agent): DisplayStatus | null {
  if (!agent) return { kind: "offline", reason: "所属服务器已删除" };
  if (agent.error) return { kind: "failed", reason: agent.error };
  if (agent.deleting) return online(agent)
    ? { kind: "pending", reason: "正在卸载并清理服务器" }
    : { kind: "offline", reason: "服务器上线后卸载并清理" };
  if (!online(agent)) return { kind: "offline",
    reason: agent.revision !== agent.applied ? "服务器上线后自动应用配置" : undefined };
  if (agent.revision !== agent.applied) return { kind: "pending", reason: "正在等待服务器确认配置" };
  const failed = agent.cores.find((core) => core.state === "error");
  return failed ? { kind: "failed", reason: failed.error || `${failed.core} 核心运行异常` } : null;
}
export function agentStatus(agent: Agent): DisplayStatus {
  return applicationIssue(agent) || { kind: agent.enabled === false || agent.revoked ? "stopped" : "enabled" };
}
export function resourceStatus(resource: Resource, agent: Agent | undefined, used: number, at = Date.now()): DisplayStatus {
  const issue = applicationIssue(agent);
  if (issue) return issue;
  if (!resource.enabled) return { kind: "stopped" };
  if (agent!.enabled === false || agent!.revoked) return { kind: "stopped", reason: "所属服务器已停止" };
  if (resource.expires_at !== null && resource.expires_at <= at) return { kind: "stopped", reason: "已到期" };
  if (resource.quota_bytes !== null && used >= resource.quota_bytes) return { kind: "stopped", reason: "流量额度已用尽" };
  const core = resource.spec.type === "forward" ? "realm" : "sing-box";
  if (agent!.cores.find((value) => value.core === core)?.state !== "running")
    return { kind: "pending", reason: `正在等待 ${core === "realm" ? "Realm" : core} 核心启动` };
  return { kind: "enabled" };
}
export const primaryAddress = (agent?: Agent) =>
  agent?.ipv4 || agent?.ipv6 || agent?.address || "";
export const defaultListen = (agent?: Agent) =>
  primaryAddress(agent).includes(":") ? "::" : "0.0.0.0";
export const forwardListen = (agent?: Agent) =>
  agent?.ipv6 ? "::" : defaultListen(agent);
export const defaultPort = (kind: Spec["type"]) =>
  kind === "shadowsocks" ? 2053 : 8443;
export interface Job {
  id: string;
  agent_id: string;
  revision: number;
  state: string;
  error: string | null;
  created_at: number;
  finished_at: number | null;
}
export interface Overview {
  agents: Agent[];
  nodes: number;
  forwards: number;
  month_total: number;
  month_traffic: Record<string, number>;
}
export interface Traffic {
  at: number;
  upload: number;
  download: number;
}
export interface Log {
  id: string;
  agent_id: string;
  resource_id: string;
  network: string;
  source: string;
  destination: string;
  domain: string;
  started_at: number;
  closed_at: number | null;
  upload: number;
  download: number;
  result: string;
}
export const bytes = (value = 0) => {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  let n = Math.max(0, Math.min(4, Math.floor(Math.log2(value) / 10)));
  if (n < 4 && Number((value / 1024 ** n).toFixed(n ? 2 : 0)) >= 1024) n++;
  return `${(value / 1024 ** n).toLocaleString("en-US", { minimumFractionDigits: n ? 2 : 0, maximumFractionDigits: n ? 2 : 0 })} ${["B", "KB", "MB", "GB", "TB"][n]}`;
};
export const time = (value: number | null, full = true) =>
  value
    ? new Intl.DateTimeFormat("zh-CN", {
        timeZone: "Asia/Shanghai",
        ...(full ? { month: "2-digit", day: "2-digit" } : {}),
        hour: "2-digit",
        minute: "2-digit",
        ...(full ? { second: "2-digit" } : {}),
        hour12: false,
      }).format(value)
    : "—";
export const online = (a: Agent) =>
  !a.revoked && a.last_seen > Date.now() - 35000;
const nameOrder = new Intl.Collator("zh-CN", { numeric: true, sensitivity: "base" });
export const byName = <T extends { id: string; name: string }>(items: readonly T[]): T[] =>
  [...items].sort((a, b) => nameOrder.compare(a.name, b.name) || a.id.localeCompare(b.id));
export const protocol = (r: Resource) =>
  r.spec.type === "vless"
    ? "Vless Reality"
    : r.spec.type === "shadowsocks"
      ? "Shadowsocks 2022"
      : r.spec.network === "tcp_udp"
        ? "TCP + UDP"
        : r.spec.network.toUpperCase();
export const address = (host: string, port: number) =>
  `${host.includes(":") ? `[${host}]` : host}:${port}`;
