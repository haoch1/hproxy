import type {
  Agent,
  Counter,
  Job,
  Log,
  Resource,
  Spec,
  Traffic,
} from "./types";
import { rangeStart } from "./traffic";
import { primaryAddress, forwardListen } from "./types";
import { demoConfig } from "./demoConfig";
import type { QuotaBaseline } from "./types";
import { quotaCounter, quotaWindow, sameCycle, validCycle } from "./quota";

// This adapter never falls through to fetch. Demo data and mutations stay in this browser.
const STORAGE_KEY = "hproxy-demo-v1";
export interface DemoFailureCase {
  title: string;
  error: string;
  page: "servers" | "nodes" | "forwards";
  agent_id: string;
  resource?: Resource;
  isNew?: boolean;
}
const failureScenarios: {
  id: string;
  title: string;
  page: DemoFailureCase["page"];
  port?: number;
  network?: "tcp" | "udp";
  error?: string;
}[] = [
  { id: "demo-port-conflict", title: "节点 · TCP 端口占用", page: "nodes", port: 18080, network: "tcp" },
  { id: "demo-node-udp-conflict", title: "节点 · UDP 端口占用", page: "nodes", port: 18081, network: "udp" },
  { id: "demo-forward-tcp-conflict", title: "转发 · TCP 端口占用", page: "forwards", port: 18080, network: "tcp" },
  { id: "demo-forward-udp-conflict", title: "转发 · UDP 端口占用", page: "forwards", port: 18081, network: "udp" },
  { id: "demo-node-start-failure", title: "节点 · 启动失败", page: "nodes", error: "sing-box 启动失败：核心进程意外退出，请重试" },
  { id: "demo-forward-start-failure", title: "转发 · 启动失败", page: "forwards", error: "Realm 启动失败：核心进程意外退出，请重试" },
  { id: "demo-core-install-failure", title: "服务器 · 核心安装失败", page: "servers", error: "sing-box 安装失败：下载连接超时，请重试" },
];
const GiB = 1024 ** 3,
  HOUR = 3600000,
  DAY = 24 * HOUR;
interface DemoState {
  version: 1;
  created: number;
  agents: Agent[];
  online: string[];
  resources: Resource[];
  counters: Record<string, Counter>;
  quota_baselines?: Record<string, QuotaBaseline>;
  jobs: Job[];
  logs: Log[];
  log_days: number;
  update_demo?: {
    checked: number;
    phase: string;
    ready_at: number;
    installed: boolean;
    installed_components?: string[];
    pending_components?: string[];
  };
}
const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));
const hex = (value: string) =>
  Uint8Array.from(value.match(/../g)!, (v) => parseInt(v, 16));
const url64 = (bytes: Uint8Array) =>
  b64(bytes).replace(/=/g, "").replace(/\+/g, "-").replace(/\//g, "_");
// Public RFC 7748 test vector: intentionally shared sample keys, never a real credential.
const sampleSpec = (): Extract<Spec, { type: "vless" }> => ({
  type: "vless",
  uuid: crypto.randomUUID(),
  private_key: url64(
    hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"),
  ),
  public_key: url64(
    hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"),
  ),
  short_id: "a1b2c3d4",
  sni: "www.bing.com",
});
const ssSpec = (): Spec => ({
  type: "shadowsocks",
  password: b64(crypto.getRandomValues(new Uint8Array(16))),
});
async function freshVless(): Promise<Spec> {
  const pair = (await crypto.subtle.generateKey({ name: "X25519" }, true, [
    "deriveBits",
  ])) as CryptoKeyPair;
  const privateKey = await crypto.subtle.exportKey("jwk", pair.privateKey);
  const publicKey = await crypto.subtle.exportKey("raw", pair.publicKey);
  return {
    ...sampleSpec(),
    private_key: privateKey.d!,
    public_key: url64(new Uint8Array(publicKey)),
    short_id: Array.from(crypto.getRandomValues(new Uint8Array(4)), (x) =>
      x.toString(16).padStart(2, "0"),
    ).join(""),
  };
}
const forwardSpec = (): Extract<Spec, { type: "forward" }> => ({
  type: "forward",
  remote_host: "203.0.113.80",
  remote_port: 8443,
  network: "tcp",
});

function seed(): DemoState {
  const now = Date.now();
  const agents: Agent[] = [
    {
      id: "demo-main",
      country_code: "JP",
      name: "主控 · 东京",
      address: "192.0.2.10",
      ipv4: "192.0.2.10",
      ipv6: "2001:db8:10::10",
      revoked: false,
      revision: 12,
      applied: 12,
      last_seen: now,
      version: "0.1.0",
      platform: "linux / x86_64",
      enrolled: true,
      error: null,
      cores: [
        { core: "sing-box", state: "running", version: "1.14.3", error: null },
        {
          core: "realm",
          state: "running",
          version: "2.9.6-hproxy.1",
          error: null,
        },
      ],
    },
    {
      id: "demo-hk",
      country_code: "HK",
      name: "香港 · NAT 01",
      address: "hk.example.com",
      ipv4: "198.51.100.24",
      ipv6: "2001:db8:24::1",
      revoked: false,
      revision: 8,
      applied: 8,
      last_seen: now,
      version: "0.1.0",
      platform: "linux / aarch64 · LXC",
      enrolled: true,
      error: null,
      cores: [
        { core: "sing-box", state: "running", version: "1.14.3", error: null },
        {
          core: "realm",
          state: "running",
          version: "2.9.6-hproxy.1",
          error: null,
        },
      ],
    },
    {
      id: "demo-sg",
      country_code: "SG",
      name: "新加坡 · 轻量",
      address: "192.0.2.30",
      revoked: false,
      revision: 5,
      applied: 5,
      last_seen: now,
      version: "0.1.0",
      platform: "linux / x86_64 · LXC",
      enrolled: true,
      error: null,
      cores: [
        { core: "sing-box", state: "running", version: "1.14.3", error: null },
      ],
    },
    {
      id: "demo-us",
      country_code: "US",
      name: "洛杉矶 · 备用",
      address: "192.0.2.40",
      revoked: false,
      revision: 3,
      applied: 3,
      last_seen: now - 42 * 60000,
      version: "0.1.0",
      platform: "linux / x86_64",
      enrolled: true,
      error: null,
      cores: [
        { core: "sing-box", state: "idle", version: "1.14.3", error: null },
      ],
    },
  ];
  function node(
    id: string,
    agent: number,
    name: string,
    port: number,
    spec: Spec,
    extra: Partial<Resource> = {},
  ): Resource {
    return {
      id,
      agent_id: agents[agent].id,
      name,
      enabled: true,
      listen_ip: "0.0.0.0",
      listen_port: port,
      public_host: agents[agent].address,
      address_bindings: { public_host: true, listen_ip: true },
      quota_bytes: 500 * GiB,
      expires_at: null,
      spec,
      ...extra,
    };
  }
  const resources: Resource[] = [
    node("demo-vless-tokyo", 0, "东京 Reality", 8443, sampleSpec()),
    node("demo-ss-tokyo", 0, "东京 SS 2022", 8388, ssSpec(), {
      quota_bytes: 200 * GiB,
    }),
    node("demo-vless-hk", 1, "香港 Reality · NAT", 8443, sampleSpec()),
    node("demo-ss-hk", 1, "香港 SS · 共享", 8388, ssSpec(), {
      quota_bytes: 100 * GiB,
      expires_at: now + 12 * DAY,
    }),
    node("demo-vless-sg", 2, "新加坡 Reality", 8443, sampleSpec()),
    node("demo-vless-us", 3, "洛杉矶备用节点", 8443, sampleSpec(), {
      enabled: false,
      quota_bytes: null,
    }),
    node("demo-forward-hk", 1, "香港 → 东京", 9443, {
      type: "forward",
      remote_host: "192.0.2.10",
      remote_port: 8443,
      network: "tcp",
    }, { address_bindings: { public_host: true, listen_ip: true, target_agent_id: agents[0].id } }),
    node(
      "demo-forward-tcpudp",
      0,
      "游戏转发 · TCP + UDP",
      27015,
      {
        type: "forward",
        remote_host: "game.example.com",
        remote_port: 27015,
        network: "tcp_udp",
      },
      { quota_bytes: 100 * GiB },
    ),
    node(
      "demo-forward-udp",
      1,
      "UDP 转发",
      5353,
      {
        type: "forward",
        remote_host: "203.0.113.53",
        remote_port: 5353,
        network: "udp",
      },
      { quota_bytes: null },
    ),
  ];
  const totals = [186.4, 42.8, 213.6, 67.2, 109.5, 0.8, 91.7, 24.3, 3.1];
  const counters = Object.fromEntries(
    resources.map((r, i) => [
      r.id,
      {
        upload: Math.round(totals[i] * GiB * 0.18),
        download: Math.round(totals[i] * GiB * 0.82),
      },
    ]),
  );
  const destinations = [
    ["github.com", "140.82.112.3:443"],
    ["docs.rs", "151.101.2.137:443"],
    ["www.wikipedia.org", "208.80.154.224:443"],
    ["www.rust-lang.org", "185.199.108.153:443"],
    ["registry.npmjs.org", "104.16.24.34:443"],
    ["www.bing.com", "204.79.197.200:443"],
  ];
  const active = resources.filter((r) => r.enabled);
  const logs: Log[] = Array.from({ length: 148 }, (_, i) => {
    const r = active[i % active.length],
      destination = destinations[i % destinations.length],
      isForward = r.spec.type === "forward";
    const started = now - (i * 7 + 2) * 60000,
      failed = i % 29 === 12;
    return {
      id: `demo-log-${i}`,
      agent_id: r.agent_id,
      resource_id: r.id,
      network:
        r.spec.type === "forward" && r.spec.network === "udp" ? "udp" : "tcp",
      source: `203.0.113.${23 + (i % 4) * 17}:${41000 + i * 3}`,
      destination:
        isForward && r.spec.type === "forward"
          ? `${r.spec.remote_host}:${r.spec.remote_port}`
          : destination[1],
      domain: isForward
        ? r.spec.type === "forward" &&
          r.spec.remote_host.includes("example.com")
          ? r.spec.remote_host
          : ""
        : destination[0],
      started_at: started,
      closed_at: i < 2 ? null : started + (failed ? 3200 : 85000 + i * 350),
      upload: failed ? 0 : 3420 + i * 1377,
      download: failed ? 0 : 185000 + i * 23571,
      result: failed
        ? "error: connect timeout"
        : i < 2
          ? "active"
          : "completed",
    };
  });
  return {
    version: 1,
    created: now,
    agents,
    online: agents.slice(0, 3).map((a) => a.id),
    resources,
    counters,
    logs,
    log_days: 7,
    jobs: agents.map((a, i) => ({
      id: `demo-job-${i}`,
      agent_id: a.id,
      revision: a.revision,
      state: "applied",
      error: null,
      created_at: now - (i + 1) * HOUR,
      finished_at: now - (i + 1) * HOUR + 1800,
    })),
  };
}

function load(): DemoState {
  try {
    const saved = JSON.parse(
      localStorage.getItem(STORAGE_KEY) || "null",
    ) as DemoState | null;
    if (
      saved?.version === 1 &&
      Array.isArray(saved.agents) &&
      Array.isArray(saved.resources) &&
      Array.isArray(saved.logs) &&
      Date.now() - saved.created < DAY
    ) {
      for (const r of saved.resources)
        delete (r as Resource & { public_port?: number }).public_port;
      for (const a of saved.agents) {
        if (a.ipv4 === undefined && /^\d+\.\d+\.\d+\.\d+$/.test(a.address))
          a.ipv4 = a.address;
        a.country_code ||= (
          {
            "demo-main": "JP",
            "demo-hk": "HK",
            "demo-sg": "SG",
            "demo-us": "US",
          } as Record<string, string>
        )[a.id];
      }
      const main = saved.agents.find((a) => a.id === "demo-main");
      if (main) {
        main.ipv6 = "2001:db8:10::10";
        main.country_code = "JP";
      }
      const hk = saved.agents.find((a) => a.id === "demo-hk");
      if (hk) {
        hk.ipv4 = "198.51.100.24";
        hk.ipv6 = "2001:db8:24::1";
        hk.country_code = "HK";
      }
      return saved;
    }
  } catch {
    /* A corrupt demo session can always be recreated. */
  }
  return seed();
}
let state = load();
function normalizeCores() {
  for (const a of state.agents) {
    a.enabled ??= true;
    a.core_management = true;
    a.quota_cycles = true;
    a.cleanup_supported = true;
    a.uninstall_supported = true;
    a.core_policy ??= a.cores
      .filter((c) => c.state !== "missing")
      .map((c) => c.core);
    for (const r of state.resources.filter((r) => r.agent_id === a.id)) {
      const core = r.spec.type === "forward" ? "realm" : "sing-box";
      if (!a.core_policy.includes(core)) a.core_policy.push(core);
    }
  }
}
normalizeCores();
function persist() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    /* Browsers with disabled storage still support an in-memory demo. */
  }
}
function heartbeat() {
  for (const a of state.agents) {
    if (state.online.includes(a.id) && !a.revoked) a.last_seen = Date.now();
  }
  state.quota_baselines ??= {};
  for (const r of state.resources) {
    const cycle = r.quota_cycle;
    if (!cycle || !validCycle(cycle)) { delete state.quota_baselines[r.id]; continue; }
    if (!state.online.includes(r.agent_id)) continue;
    const start = quotaWindow(cycle)[0], total = state.counters[r.id] || { upload: 0, download: 0 };
    const baseline = state.quota_baselines[r.id] ??= { cycle, start, counter: { upload: 0, download: 0 } };
    if (!sameCycle(baseline.cycle, cycle)) { baseline.cycle = cycle; baseline.start = start; }
    else if (start > baseline.start) { baseline.start = start; baseline.counter = { ...total }; }
  }
}
function resourceCounters() {
  return Object.fromEntries(state.resources.map((r) => [r.id, quotaCounter(r, state.counters[r.id] || { upload: 0, download: 0 }, state.quota_baselines?.[r.id])]));
}
function getAgent(id: string) {
  const a = state.agents.find((a) => a.id === id);
  if (!a) throw new Error("服务器不存在");
  return a;
}
function bump(agentId: string, simulatedError?: string) {
  const a = getAgent(agentId);
  normalizeCores();
  a.revision++;
  const connected = state.online.includes(agentId) && !a.revoked;
  const assigned = state.resources.filter(
    (r) => r.agent_id === a.id && r.enabled && a.enabled !== false,
  );
  // Only the dedicated demo servers simulate external processes holding ports.
  const scenario = failureScenarios.find((s) => s.id === agentId && s.network);
  const conflict = scenario && assigned.find((r) => r.listen_port === scenario.port && protocols(r).includes(scenario.network!));
  if (connected) a.error = simulatedError || (conflict
    ? `${scenario!.network!.toUpperCase()} 监听端口 ${scenario!.port} 已被占用，请修改端口后重试（${conflict.name}）`
    : null);
  const applied = connected && !a.error;
  if (applied) {
    a.applied = a.revision;
    a.cores = [
      {
        core: "sing-box",
        state: !a.core_policy?.includes("sing-box")
          ? "missing"
          : assigned.some((r) => r.spec.type !== "forward")
            ? "running"
            : "idle",
        version: a.core_policy?.includes("sing-box") ? "1.14.3" : "",
        error: null,
      },
      {
        core: "realm",
        state: !a.core_policy?.includes("realm")
          ? "missing"
          : assigned.some((r) => r.spec.type === "forward")
            ? "running"
            : "idle",
        version: a.core_policy?.includes("realm") ? "2.9.6-hproxy.1" : "",
        error: null,
      },
    ];
  }
  state.jobs.unshift({
    id: crypto.randomUUID(),
    agent_id: a.id,
    revision: a.revision,
    state: connected && a.error ? "failed" : applied ? "applied" : "pending",
    error: a.error,
    created_at: Date.now(),
    finished_at: connected ? Date.now() : null,
  });
  state.jobs = state.jobs.slice(0, 100);
}
function protocols(r: Resource) {
  return r.spec.type === "vless" ||
    (r.spec.type === "forward" && r.spec.network === "tcp")
    ? ["tcp"]
    : r.spec.type === "forward" && r.spec.network === "udp"
      ? ["udp"]
      : ["tcp", "udp"];
}
function normalizeAddresses(r: Resource) {
  const bindings = r.address_bindings;
  if (!bindings) return;
  const a = getAgent(r.agent_id);
  if (bindings.public_host) r.public_host = primaryAddress(a) || r.public_host;
  if (bindings.listen_ip && primaryAddress(a)) r.listen_ip = forwardListen(a);
  if (bindings.target_agent_id) {
    const target = getAgent(bindings.target_agent_id);
    if (r.spec.type !== "forward") throw new Error("只有转发规则可以关联落地服务器");
    if (primaryAddress(target)) {
      r.spec.remote_host = primaryAddress(target);
      r.spec.remote_host_v6 = target.ipv4 && target.ipv6 ? target.ipv6 : null;
    }
  }
}
function validateResource(r: Resource, all: Resource[]) {
  if (r.quota_cycle && !validCycle(r.quota_cycle)) throw new Error("流量周期或开始日期无效");
  const a = getAgent(r.agent_id);
  if (a.revoked) throw new Error("此服务器已撤销连接");
  if (!r.name?.trim() || r.name.length > 100)
    throw new Error("名称须为 1–100 个字符");
  if (!r.listen_ip || !r.public_host || /[\s/;"'`]/.test(r.public_host))
    throw new Error("请填写有效的监听地址与公网地址");
  for (const port of [
    r.listen_port,
    ...(r.spec.type === "forward" ? [r.spec.remote_port] : []),
  ])
    if (!Number.isInteger(port) || port < 1 || port > 65535)
      throw new Error("端口须为 1–65535");
  if (r.spec.type === "forward" && !r.spec.remote_host)
    throw new Error("请填写地址");
  if (r.spec.type === "shadowsocks") {
    let valid = false;
    try { valid = atob(r.spec.password).length === 16; } catch { /* Invalid Base64. */ }
    if (!valid) throw new Error("SS 2022 密钥须为 16 字节 Base64，请重新生成连接参数后重试");
  }
  if (
    r.quota_bytes !== null &&
    (!Number.isFinite(r.quota_bytes) || r.quota_bytes <= 0)
  )
    throw new Error("流量额度必须大于 0");
  if (
    r.enabled &&
    all.some(
      (x) =>
        x.id !== r.id &&
        x.agent_id === r.agent_id &&
        x.enabled &&
        x.listen_port === r.listen_port &&
        protocols(x).some((p) => protocols(r).includes(p)),
    )
  )
    throw new Error(`监听端口 ${r.listen_port} 已被此服务器上的其他配置占用，请修改端口后重试`);
}

function simulateFailures(): DemoFailureCase[] {
  const cases: DemoFailureCase[] = [];
  for (const [index, scenario] of failureScenarios.entries()) {
    const previous = state.agents.find((a) => a.id === scenario.id);
    const agent: Agent = {
      id: scenario.id, name: `失败示例 · ${scenario.title}`, address: `192.0.2.${50 + index}`,
      enabled: true, enrolled: true, revoked: false,
      revision: previous?.revision ?? 1, applied: previous?.applied ?? 1,
      last_seen: Date.now(), version: "0.1.0", platform: "linux / x86_64 · 示例", error: null,
      core_policy: ["sing-box", "realm"],
      cores: ["sing-box", "realm"].map((core) => ({
        core, state: scenario.page === "servers" ? "missing" : "idle",
        version: scenario.page === "servers" ? "" : core === "sing-box" ? "1.14.3" : "2.9.6-hproxy.1", error: null,
      })),
    };
    state.agents = [agent, ...state.agents.filter((a) => a.id !== agent.id)];
    if (!state.online.includes(agent.id)) state.online.push(agent.id);
    const resource: Resource | undefined = scenario.page === "servers" ? undefined : {
      id: scenario.id === "demo-port-conflict" ? "demo-node-port-conflict" : `${agent.id}-resource`,
      agent_id: agent.id, name: scenario.title, enabled: true,
      listen_ip: "0.0.0.0", listen_port: scenario.port || 18443, public_host: agent.address,
      quota_bytes: null, expires_at: null,
      spec: scenario.page === "forwards"
        ? { ...forwardSpec(), network: scenario.network || "tcp" } : ssSpec(),
    };
    if (resource) {
      state.resources = [resource, ...state.resources.filter((r) => r.id !== resource.id)];
      state.counters[resource.id] = { upload: 0, download: 0 };
    }
    bump(agent.id, scenario.error);
    cases.push({ title: scenario.title, page: scenario.page, agent_id: agent.id, error: agent.error!, resource });
  }
  // Rejected drafts use the normal validator and never enter the saved resource list.
  const agent = state.agents.find((a) => a.enrolled && a.enabled !== false && !a.revoked
    && !a.deleting && !a.error && state.online.includes(a.id)) || getAgent("demo-node-start-failure");
  for (const forward of [false, true]) {
    const draft: Resource = {
      id: crypto.randomUUID(), agent_id: agent.id,
      name: forward ? "转发 · 创建参数无效" : "节点 · 创建参数无效", enabled: true,
      listen_ip: "0.0.0.0", listen_port: 18444, public_host: agent.address,
      quota_bytes: null, expires_at: null,
      spec: forward ? { type: "forward", remote_host: "", remote_port: 8443, network: "tcp" }
        : { type: "shadowsocks", password: "invalid-demo-key" },
    };
    try { validateResource(draft, state.resources); }
    catch (error) {
      cases.push({ title: draft.name, error: (error as Error).message,
        page: forward ? "forwards" : "nodes", agent_id: agent.id, resource: draft, isNew: true });
    }
  }
  return cases;
}

function traffic(params: URLSearchParams): Traffic[] {
  const resources = state.resources.filter(
    (r) =>
      (!params.get("agent_id") || r.agent_id === params.get("agent_id")) &&
      (!params.get("resource_id") || r.id === params.get("resource_id")) &&
      r.spec.type !== "forward",
  );
  const totals = resources.reduce(
    (n, r) => ({
      upload: n.upload + (state.counters[r.id]?.upload || 0),
      download: n.download + (state.counters[r.id]?.download || 0),
    }),
    { upload: 0, download: 0 },
  );
  const hour = Math.floor(Date.now() / HOUR) * HOUR;
  const weights = Array.from({ length: 720 }, (_, i) => {
    const h = (i + 8) % 24;
    return (0.5 + Math.sin(i * 0.67) ** 2) * (h > 9 && h < 23 ? 1.7 : 0.55);
  });
  const weight = weights.reduce((n, x) => n + x, 0),
    from = Number(params.get("from") || hour - 30 * DAY),
    to = Number(params.get("to") || Date.now());
  return weights
    .map((w, i) => ({
      at: hour - (719 - i) * HOUR,
      upload: Math.round((totals.upload * w) / weight),
      download: Math.round((totals.download * w) / weight),
    }))
    .filter((r) => r.at >= from && r.at <= to);
}
function queryLogs(params: URLSearchParams) {
  const q = (params.get("q") || "").trim().toLowerCase(),
    source = (params.get("source") || "").trim().toLowerCase(),
    target = (params.get("target") || "").trim().toLowerCase(),
    network = params.get("network") || "",
    status = params.get("status") || "",
    from = Number(params.get("from") || 0),
    to = Number(params.get("to") || Date.now());
  return state.logs.filter((l) => {
    const port = l.destination.match(/^(?:\[[^\]]+\]|[^:]+):(\d{1,5})$/)?.[1];
    const address = l.domain
      ? `${l.domain}${port ? `:${port}` : ""}`
      : l.destination;
    const failed = l.result.startsWith("error");
    return (
      (!params.get("agent_id") || l.agent_id === params.get("agent_id")) &&
      (!params.get("resource_id") ||
        l.resource_id === params.get("resource_id")) &&
      state.resources.find((r) => r.id === l.resource_id)?.spec.type !== "forward" &&
      l.started_at >= Date.now() - state.log_days * DAY &&
      l.started_at >= from &&
      l.started_at <= to &&
      [l.source, l.destination, address].some((v) =>
        v.toLowerCase().includes(q),
      ) &&
      l.source.toLowerCase().includes(source) &&
      [l.destination, address].some((v) => v.toLowerCase().includes(target)) &&
      (!network || l.network === network) &&
      (!status ||
        (status === "failed"
          ? failed
          : status === "active"
            ? !failed && !l.closed_at
            : !failed && !!l.closed_at))
    );
  });
}
export async function demoApi<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  heartbeat();
  const url = new URL(path, "https://demo.invalid"),
    method = options.method || "GET",
    parts = url.pathname.split("/").filter(Boolean);
  const body = options.body ? JSON.parse(String(options.body)) : {};
  let result: unknown = { ok: true };
  if (parts[0] === "me" || parts[0] === "login")
    result = { csrf: "local-demo" };
  else if (parts[0] === "logout") result = { ok: true };
  else if (parts[0] === "demo" && parts[1] === "reset" && method === "POST") {
    state = seed();
    normalizeCores();
    persist();
  } else if (parts[0] === "demo" && parts[1] === "failures" && method === "POST") {
    result = simulateFailures();
  } else if (parts[0] === "overview") {
    const agents = state.agents.filter((a) => a.enrolled);
    const resources = state.resources.filter((r) =>
      agents.some((a) => a.id === r.agent_id),
    );
    const month_traffic = Object.fromEntries(
      agents.map((a) => [
        a.id,
        traffic(
          new URLSearchParams({
            from: String(rangeStart("month")),
            agent_id: a.id,
          }),
        ).reduce((n, r) => n + r.upload + r.download, 0),
      ]),
    );
    result = {
      agents,
      nodes: resources.filter((r) => r.spec.type !== "forward").length,
      forwards: resources.filter((r) => r.spec.type === "forward").length,
      month_total: Object.values(month_traffic).reduce(
        (n, value) => n + value,
        0,
      ),
      month_traffic,
    };
  } else if (parts[0] === "generate")
    result =
      parts[1] === "vless"
        ? await freshVless()
        : parts[1] === "shadowsocks"
          ? ssSpec()
          : forwardSpec();
  else if (parts[0] === "agents") {
    if (method === "GET" && parts.length === 1) {
      result = state.agents.filter((a) => a.enrolled);
    } else if (method === "POST" && parts.length === 1) {
      if (!String(body.name || "").trim()) throw new Error("请填写服务器名称");
      const cores: string[] = body.cores ?? ["sing-box", "realm"];
      if (!Array.isArray(cores) || cores.length > 2 || new Set(cores).size !== cores.length
        || cores.some((core) => !["sing-box", "realm"].includes(core)))
        throw new Error("核心选择无效");
      const id = crypto.randomUUID();
      state.agents.push({
        id,
        name: body.name.trim(),
        address: body.address || "",
        revoked: false,
        enabled: true,
        core_management: true,
        quota_cycles: true,
        core_policy: [...cores],
        revision: 1,
        applied: 0,
        last_seen: 0,
        version: "0.1.0",
        platform: "linux / x86_64 · 示例",
        enrolled: false,
        error: null,
        cores: [],
      });
      result = {
        agent_id: id,
        token: "demo-only-invalid-token",
        expires_at: Date.now() + 600000,
      };
    } else {
      const a = getAgent(parts[1]);
      if (method === "GET" && parts[2] === "configs") {
        if (!state.online.includes(a.id)) throw new Error("服务器已离线，连接后可查看配置");
        result = demoConfig(parts[3], a, state.resources, resourceCounters());
      } else if (method === "PUT" && parts[2] === "cores") {
        if (!state.online.includes(a.id) || a.revoked)
          throw new Error("服务器离线，连接后可管理核心");
        const core = parts[3];
        if (!["sing-box", "realm"].includes(core))
          throw new Error("核心名称无效");
        if (
          !body.installed &&
          state.resources.some(
            (r) =>
              r.agent_id === a.id &&
              (r.spec.type === "forward" ? "realm" : "sing-box") === core,
          )
        )
          throw new Error("请先删除使用此核心的节点或转发");
        a.core_policy = (a.core_policy || []).filter((c) => c !== core);
        if (body.installed) a.core_policy.push(core);
        bump(a.id);
      } else if (method === "PUT" && parts[2] === "state") {
        a.enabled = !!body.enabled;
        a.revoked = false;
        bump(a.id);
      } else if (method === "PUT") {
        a.name = body.name.trim();
        a.address = body.address || "";
      } else if (method === "DELETE") {
        if (parts[2] === "record" || state.online.includes(a.id)) state.agents = state.agents.filter((x) => x.id !== a.id);
        else { a.deleting = true; a.enabled = false; a.core_policy = []; }
        state.resources = state.resources.filter((r) => r.agent_id !== a.id);
        state.jobs = state.jobs.filter((j) => j.agent_id !== a.id);
        state.online = state.online.filter((id) => id !== a.id);
      } else if (parts[2] === "simulate-install") {
        a.enrolled = true;
        a.ipv4 = "203.0.113.42";
        a.ipv6 = "2001:db8:42::1";
        a.address = a.ipv4;
        a.last_seen = Date.now();
        state.online.push(a.id);
        bump(a.id);
      } else if (parts[2] === "restart") bump(a.id);
      else if (parts[2] === "revoke") {
        a.revoked = true;
        a.last_seen = 0;
        state.online = state.online.filter((id) => id !== a.id);
      } else if (parts[2] === "enrollment")
        result = {
          agent_id: a.id,
          token: "demo-only-invalid-token",
          expires_at: Date.now() + 600000,
        };
      else throw new Error("此操作在示例模式中不可用");
    }
  } else if (parts[0] === "resources") {
    if (method === "GET" && parts.length === 1)
      result = {
        resources: state.resources.filter((r) =>
          state.agents.some((a) => a.enrolled && a.id === r.agent_id),
        ),
        counters: resourceCounters(),
      };
    else if (method === "GET" && parts[2] === "link") {
      const r = state.resources.find((r) => r.id === parts[1]);
      if (!r) throw new Error("配置不存在");
      const host = r.public_host.includes(":")
        ? `[${r.public_host}]`
        : r.public_host;
      result = {
        link:
          r.spec.type === "vless"
            ? `vless://${r.spec.uuid}@${host}:${r.listen_port}?encryption=none&security=reality&type=tcp&flow=xtls-rprx-vision&sni=${encodeURIComponent(r.spec.sni)}&pbk=${r.spec.public_key}&sid=${r.spec.short_id}#${encodeURIComponent(r.name)}`
            : r.spec.type === "shadowsocks"
              ? `ss://${btoa(`2022-blake3-aes-128-gcm:${r.spec.password}`)}@${host}:${r.listen_port}#${encodeURIComponent(r.name)}`
              : "",
      };
    } else if (method === "POST" && parts[1] === "import") {
      if (
        !Array.isArray(body.resources) ||
        !body.resources.length ||
        body.resources.length > 512
      )
        throw new Error("一次导入 1–512 条配置");
      const incoming = body.resources as Resource[],
        all = [...state.resources, ...incoming];
      for (const r of incoming) {
        normalizeAddresses(r);
        if (state.resources.some((x) => x.id === r.id))
          throw new Error("配置 ID 已存在");
        validateResource(r, all);
      }
      state.resources = all;
      for (const id of new Set(incoming.map((r) => r.agent_id))) bump(id);
      result = { imported: incoming.length };
    } else if (method === "POST" || method === "PUT") {
      const r = body as Resource,
        exists = state.resources.some((x) => x.id === r.id);
      if (method === "POST" && exists) throw new Error("配置已存在");
      if (method === "PUT" && !exists) throw new Error("配置不存在");
      normalizeAddresses(r);
      validateResource(r, state.resources);
      if (exists) state.resources = state.resources.map((x) => x.id === r.id ? r : x);
      else state.resources.push(r);
      state.counters[r.id] ??= { upload: 0, download: 0 };
      bump(r.agent_id);
      result = { id: r.id, revision: getAgent(r.agent_id).revision };
    } else if (method === "DELETE") {
      const r = state.resources.find((r) => r.id === parts[1]);
      if (!r) throw new Error("配置不存在");
      state.resources = state.resources.filter((x) => x.id !== r.id);
      bump(r.agent_id);
    } else throw new Error("此操作在示例模式中不可用");
  } else if (parts[0] === "jobs") result = state.jobs;
  else if (parts[0] === "logs") {
    const page_size = Number(url.searchParams.get("page_size") || 20);
    if (![20, 50, 100].includes(page_size))
      throw new Error("每页条数须为 20、50 或 100");
    const all = queryLogs(url.searchParams),
      pages = Math.max(1, Math.ceil(all.length / page_size)),
      page = Math.min(
        pages,
        Math.max(1, Number(url.searchParams.get("page") || 1)),
      );
    result = {
      rows: all.slice((page - 1) * page_size, page * page_size),
      has_more: page < pages,
      total: all.length,
      pages,
      page_size,
      page,
    };
  } else if (parts[0] === "traffic")
    result = { rows: traffic(url.searchParams) };
  else if (parts[0] === "settings") {
    if (method === "PUT") {
      if (
        !Number.isInteger(body.log_days) ||
        body.log_days < 1 ||
        body.log_days > 365
      )
        throw new Error("日志保留期须为 1–365 天");
      state.log_days = body.log_days;
      state.logs = state.logs.filter((log) => log.started_at >= Date.now() - state.log_days * DAY);
    } else
      result = {
        log_days: state.log_days,
        timezone: "Asia/Shanghai",
        sing_box: "1.14.3",
        realm: "2.9.6-hproxy.1",
        version: "0.1.0",
      };
  } else if (parts[0] === "updates") {
    const update = (state.update_demo ||= {
      checked: 0,
      phase: "idle",
      ready_at: 0,
      installed: false,
    });
    if (update.ready_at && Date.now() >= update.ready_at) {
      if (update.phase === "downloading") {
        update.installed_components = [...new Set([...(update.installed_components || []), ...(update.pending_components || [])])];
        update.pending_components = [];
      }
      update.phase = "idle";
      update.ready_at = 0;
      persist();
    }
    // A compatibility release can update hproxy/Realm while retaining sing-box.
    // Keep the previously installed demo version when loading older saved sessions.
    const components = () => [
      ["hproxy", "hproxy", "0.1.0", "0.1.1", "0.1.1"],
      ["sing_box", "sing-box", "1.14.3", "1.14.3", "1.14.4"],
      ["realm", "Realm", "2.9.6-hproxy.1", "2.9.6-hproxy.2", "2.9.6-hproxy.2"],
    ].map(([key, name, base, latest, installed]) => {
      const updated = update.installed || update.installed_components?.includes(key);
      const current = updated ? installed : base;
      return { key, name, current, latest: update.checked ? latest : null,
        state: !update.checked ? "unchecked" : current === latest ? "current" : updated ? "newer" : "available" };
    });
    if (parts[1] === "check" && method === "POST") {
      update.checked = Date.now();
      update.phase = "checking";
      update.ready_at = Date.now() + 800;
    } else if (parts[1] === "apply" && method === "POST") {
      if (!update.checked || body.build !== "2".repeat(64))
        throw new Error("请先检查更新");
      const component = body.component || "all";
      if (!["all", "hproxy", "sing_box", "realm"].includes(component)) throw new Error("组件名称无效");
      if (update.phase !== "idle" && update.phase !== "failed") throw new Error("已有更新任务正在执行");
      update.pending_components = components().filter((item) => item.state === "available" && (component === "all" || item.key === component)).map((item) => item.key);
      if (!update.pending_components.length) throw new Error("暂无可更新的组件");
      update.phase = "downloading";
      update.ready_at = Date.now() + 1800;
    }
    result = {
      phase: update.phase,
      error: null,
      source_configured: true,
      can_update: true,
      checked_at: update.checked || null,
      build: update.checked ? "2".repeat(64) : null,
      components: components(),
      agents: [],
    };
  } else if (parts[0] === "password")
    throw new Error("示例模式无需登录，不修改真实管理员密码");
  else throw new Error("此操作在示例模式中不可用");
  if (method !== "GET") persist();
  return structuredClone(result) as T;
}
