import { ActionMenu, ActionMenuPower } from "./ActionMenu";
import TrafficUsage from "./TrafficUsage";
import { transitionContent } from "./contentTransition";
import Select from "./Select";
import { ThemeControl } from "./theme";
import Brand from "./Brand";
import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import {
  HardDrive,
  Infinity as InfinityIcon,
  ArrowRightLeft,
  ScrollText,
  ChartLine,
  Settings,
  Search,
  Check,
  LogOut,
  Menu,
  Link2,
  Trash2,
  Pencil,
  Copy,
  FlaskConical,
  Info,
} from "lucide-react";
import ResourceEditor from "./ResourceEditor";
import TrafficPage from "./TrafficPage";
import Logs from "./Logs";
import ServerFlag from "./ServerFlag";
import SettingsPage from "./SettingsPage";
import CoreManager from "./CoreManager";
import { cycleLabel, quotaDay, quotaWindow } from "./quota";
import { api, post, put, remove, setCsrf, demoMode, checkUpdatesOnEntry } from "./api";
import type { DemoFailureCase } from "./demo";
import { nonCredentialAutofill } from "./formAutofill";
import type { Agent, Resource, Spec, Counter, Overview, DisplayStatus } from "./types";
import {
  address,
  bytes,
  online,
  byName,
  protocol,
  primaryAddress,
  defaultListen,
  forwardListen,
  defaultPort,
  applicationIssue,
  agentStatus,
  resourceStatus,
  statusLabels,
} from "./types";
import { Confirm, Empty, ErrorText, Field, Modal, PendingButton, Submit } from "./components";

const pages = [
  ["servers", "服务器", HardDrive],
  ["nodes", "代理节点", InfinityIcon],
  ["forwards", "端口转发", ArrowRightLeft],
  ["logs", "连接日志", ScrollText],
  ["traffic", "流量统计", ChartLine],
  ["settings", "设置", Settings],
] as const;
type Page = (typeof pages)[number][0];
type Confirmation = {
  title: string;
  description: string;
  danger?: boolean;
  action: () => Promise<void>;
};

export function App() {
  const [auth, setAuth] = useState<boolean | null>(null),
    [page, setPage] = useState<Page>("servers");
  const [sidebar, setSidebar] = useState(false);
  const [overview, setOverview] = useState<Overview>({
    agents: [],
    nodes: 0,
    forwards: 0,
    month_total: 0,
    month_traffic: {},
  });
  const [resources, setResources] = useState<Resource[]>([]),
    [counters, setCounters] = useState<Record<string, Counter>>({});
  const reportedErrors = useRef(new Map<string, string>());
  const refreshRequest = useRef(0);
  const checkedUpdates = useRef(false);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [toast, setToast] = useState(""),
    [loaded, setLoaded] = useState(false);
  const [addAgent, setAddAgent] = useState(false),
    [enrollment, setEnrollment] = useState<{
      agent_id: string;
      token: string;
      expires_at: number;
      controller?: string;
    } | null>(null);
  const [demoFailures, setDemoFailures] = useState<DemoFailureCase[] | null>(null);
  const [detail, setDetail] = useState<string | null>(null),
    [editor, setEditor] = useState<{
      resource: Resource;
      isNew: boolean;
      isCopy?: boolean;
      fixedAgentId?: string;
      initialError?: string;
    } | null>(null);
  const [confirm, setConfirm] = useState<Confirmation | null>(null),
    [link, setLink] = useState<string | null>(null),
    [needsServer, setNeedsServer] = useState(false);
  useEffect(() => {
    const change = (event?: HashChangeEvent) => {
      const p = window.location.hash.slice(1);
      if (!pages.some((x) => x[0] === p)) {
        window.history.replaceState(null, "", "#servers");
        setPage("servers");
      } else if (event) transitionContent(() => setPage(p as Page));
      else setPage(p as Page);
      setSidebar(false);
      window.scrollTo({ top: 0 });
    };
    change();
    window.addEventListener("hashchange", change);
    const expired = () => setAuth(false);
    window.addEventListener("session-expired", expired);
    api<{ csrf: string }>("/me")
      .then((r) => {
        setCsrf(r.csrf);
        setAuth(true);
      })
      .catch(() => setAuth(false));
    return () => {
      window.removeEventListener("hashchange", change);
      window.removeEventListener("session-expired", expired);
    };
  }, []);
  useEffect(() => {
    if (!auth) { checkedUpdates.current = false; return; }
    if (checkedUpdates.current) return;
    checkedUpdates.current = true;
    void checkUpdatesOnEntry().catch((e) => setToast(`检查更新失败：${e.message}`));
  }, [auth]);
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(""), 3500);
    return () => clearTimeout(timer);
  }, [toast]);
  const refresh = useCallback(async () => {
    const request = ++refreshRequest.current;
    try {
      const [o, r] = await Promise.all([
        api<Overview>("/overview"),
        api<{ resources: Resource[]; counters: Record<string, Counter> }>(
          "/resources",
        ),
      ]);
      if (request !== refreshRequest.current) return;
      const errors = new Map<string, string>();
      for (const agent of o.agents) {
        const issue = applicationIssue(agent);
        if (issue?.kind !== "failed" || !issue.reason) continue;
        const key = `${agent.revision}:${issue.reason}`;
        errors.set(agent.id, key);
        if (reportedErrors.current.get(agent.id) !== key) setToast(`${agent.name}：${issue.reason}`);
      }
      reportedErrors.current = errors;
      setOverview({ ...o, agents: byName(o.agents) });
      setResources(byName(r.resources));
      setCounters(r.counters);
      setLoaded(true);
      setError("");
    } catch (e) {
      if (request === refreshRequest.current) setError((e as Error).message);
    }
  }, []);
  const applying = overview.agents.some((agent) => agent.revision !== agent.applied && !agent.error)
    || resources.some((resource) => resourceStatus(resource, overview.agents.find((agent) => agent.id === resource.agent_id),
      (counters[resource.id]?.upload || 0) + (counters[resource.id]?.download || 0)).kind === "pending");
  useEffect(() => {
    if (!auth) return;
    void refresh();
    const timer = setInterval(() => void refresh(), applying ? 1000 : 5000);
    return () => clearInterval(timer);
  }, [auth, refresh, applying]);
  const action = async (fn: () => Promise<unknown>, message = "已保存") => {
    setBusy(true);
    try {
      await fn();
      setToast(message);
      await refresh();
    } catch (e) {
      setToast((e as Error).message);
      throw e;
    } finally {
      setBusy(false);
    }
  };
  const simulateFailures = async () => {
    setBusy(true);
    try {
      const cases = await post<DemoFailureCase[]>("/demo/failures");
      await refresh();
      setDemoFailures(cases);
    } catch (e) {
      setToast((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const create = async (
    kind: "vless" | "shadowsocks" | "forward",
    agentId?: string,
  ) => {
    const target = agentId
      ? overview.agents.find((a) => a.id === agentId)
      : overview.agents.find((a) => !a.deleting && online(a)) || overview.agents.find((a) => !a.revoked && !a.deleting);
    if (!target || target.deleting) {
      if (agentId) setToast("服务器已不存在，请刷新后重试");
      else setNeedsServer(true);
      return;
    }
    setBusy(true);
    try {
      const spec = await post<Spec>(`/generate/${kind}`);
      let targetAgentId: string | undefined;
      if (spec.type === "forward") {
        const a =
          overview.agents.find(
            (a) => a.id !== target.id && !a.revoked && primaryAddress(a),
          ) || target;
        targetAgentId = a.id;
        spec.remote_host = primaryAddress(a);
        spec.remote_host_v6 = a.ipv4 && a.ipv6 ? a.ipv6 : null;
        spec.remote_port = defaultPort("forward");
      }
      setEditor({
        isNew: true,
        fixedAgentId: agentId,
        resource: {
          id: crypto.randomUUID(),
          agent_id: target.id,
          name: "",
          enabled: true,
          listen_ip:
            kind === "forward" ? forwardListen(target) : defaultListen(target),
          listen_port: defaultPort(kind),
          public_host: primaryAddress(target),
          address_bindings: { public_host: true, listen_ip: true, target_agent_id: targetAgentId },
          quota_bytes: null,
          expires_at: null,
          spec,
        },
      });
    } catch (e) {
      setToast((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const confirmAction = (
    title: string,
    description: string,
    fn: () => Promise<unknown>,
    danger = false,
  ) =>
    setConfirm({
      title,
      description,
      danger,
      action: async () => {
        await action(fn, "操作已提交");
      },
    });
  const showLink = async (r: Resource) => {
    try {
      const data = await api<{ link: string }>(`/resources/${r.id}/link`);
      setLink(data.link);
    } catch (e) {
      setToast((e as Error).message);
    }
  };
  const toggleAgent = (agent: Agent) => {
    if (!online(agent)) return;
    const enabled = agent.enabled === false || agent.revoked;
    void action(
      () => put(`/agents/${agent.id}/state`, { enabled }),
      enabled ? "启动指令已下发" : "停止指令已下发",
    ).catch(() => {});
  };
  const deleteAgent = (agent: Agent) =>
    confirmAction(
      "删除服务器",
      "卸载子机的 hproxy agent、核心与服务，清理其安装和数据目录；离线时待上线清理，主控历史数据保留",
      async () => {
        await remove(`/agents/${agent.id}`);
        setDetail(null);
      },
      true,
    );
  const forgetAgent = (agent: Agent) =>
    confirmAction(
      "移除服务器记录",
      "仅在已于子机停止并卸载 hproxy agent 或删除容器后使用。此操作只移除面板记录和下发配置，不清理远端文件；主控历史流量仍保留。",
      async () => {
        await remove(`/agents/${agent.id}/record`);
        setDetail(null);
      },
      true,
    );
  if (auth === null)
    return (
      <div className="loading-screen">
        <span className="wordmark">
          <Brand />
        </span>
        <p>正在连接控制台…</p>
      </div>
    );
  if (!auth)
    return (
      <Login
        onLogin={(csrf) => {
          setCsrf(csrf);
          setAuth(true);
        }}
      />
    );
  const [, title, PageIcon] = pages.find((p) => p[0] === page)!;
  const selectedAgent = overview.agents.find((a) => a.id === detail);
  const selectedStatus = selectedAgent ? agentStatus(selectedAgent) : null;
  const totalOnline = overview.agents.filter(online).length;
  return (
    <div className="app-shell">
      <aside className={`sidebar ${sidebar ? "open" : ""}`}>
        <a href="#servers" className="wordmark">
          <Brand />
        </a>
        <nav>
          {pages.map(([id, label, Icon]) => (
            <a
              key={id}
              href={`#${id}`}
              className={page === id ? "nav-item active" : "nav-item"}
              aria-current={page === id ? "page" : undefined}
            >
              <Icon />
              {label}
            </a>
          ))}
        </nav>
        <div className="side-appearance"><ThemeControl compact /></div>
      </aside>
      {sidebar && (
        <button
          className="sidebar-shade"
          onClick={() => setSidebar(false)}
          aria-label="收起导航"
        />
      )}
      <div className="main-shell">
        <header className="topbar">
          <button
            className="icon-button mobile-menu"
            aria-label="打开导航"
            onClick={() => setSidebar(!sidebar)}
          >
            <Menu />
          </button>
          <div className="topbar-right">
            {!demoMode && (
              <a className="demo-entry" href="/demo/#servers">
                <FlaskConical />
                查看示例
              </a>
            )}
            <button
              className="icon-button"
              aria-label={demoMode ? "退出示例" : "退出登录"}
              onClick={() => {
                if (demoMode) {
                  window.location.assign("/");
                  return;
                }
                void post("/logout")
                  .then(() => setAuth(false))
                  .catch((e) => setToast(e.message));
              }}
            >
              <LogOut />
            </button>
          </div>
        </header>
        {demoMode && (
          <div className="demo-banner">
            <div>
              <FlaskConical />
              <strong>示例模式</strong>
            </div>
            <div>
              <button disabled={busy} onClick={() => void simulateFailures()}>
                模拟失败案例
              </button>
              <button
                onClick={() =>
                  void post("/demo/reset")
                    .then(() => window.location.reload())
                    .catch((e) => setToast(e.message))
                }
              >
                重置示例
              </button>
              <a href="/">退出示例</a>
            </div>
          </div>
        )}
        <main className="page-content">
          <div className="page-head">
            <div>
              <h1><PageIcon aria-hidden="true" />{title}</h1>
            </div>
            <div className="head-actions">
              {page === "servers" && (
                <button className="primary" onClick={() => setAddAgent(true)}>
                  添加服务器
                </button>
              )}
              {page === "nodes" && (
                <PendingButton
                  className="primary"
                  busy={busy}
                  disabled={!loaded}
                  onClick={() => void create("vless")}
                >
                  创建节点
                </PendingButton>
              )}
              {page === "forwards" && (
                <PendingButton
                  className="primary"
                  busy={busy}
                  disabled={!loaded}
                  onClick={() => void create("forward")}
                >
                  创建转发
                </PendingButton>
              )}
            </div>
          </div>
          <ErrorText error={error} />
          {!loaded ? (
            <div className="panel loading-content">正在读取服务器与配置…</div>
          ) : (
            <>
              {page === "servers" && (
                <>
                  <div className="summary-strip">
                    <Summary
                      label="在线服务器"
                      value={
                        <>
                          {totalOnline}
                          <span className="metric-suffix">
                            {" "}
                            / {overview.agents.length}
                          </span>
                        </>
                      }
                    />
                    <Summary label="代理节点" value={overview.nodes} />
                    <Summary label="转发规则" value={overview.forwards} />
                    <Summary
                      label="本月合计流量"
                      value={bytes(overview.month_total)}
                    />
                  </div>
                  <ServerTable
                    agents={overview.agents}
                    resources={resources}
                    monthlyTraffic={overview.month_traffic}
                    onManage={setDetail}
                    onCreate={(kind, id) => void create(kind, id)}
                    onToggle={toggleAgent}
                    onDelete={deleteAgent}
                    onForget={forgetAgent}
                    busy={busy}
                    onAdd={() => setAddAgent(true)}
                  />
                </>
              )}
              {(page === "nodes" || page === "forwards") && (
                <ResourceTable
                  resources={resources.filter(
                    (r) =>
                      (r.spec.type === "forward") === (page === "forwards"),
                  )}
                  agents={overview.agents}
                  counters={counters}
                  onCreate={() =>
                    void create(page === "nodes" ? "vless" : "forward")
                  }
                  onEdit={(r) =>
                    setEditor({ resource: structuredClone(r), isNew: false })
                  }
                  onCopy={(r) => setEditor({
                    resource: { ...structuredClone(r), id: crypto.randomUUID(), listen_port: 0 },
                    isNew: true,
                    isCopy: true,
                  })}
                  onLink={showLink}
                  onDelete={(r) =>
                    confirmAction(
                      "删除配置",
                      `“${r.name}”将从服务器移除，现有连接可能中断`,
                      () => remove(`/resources/${r.id}`),
                      true,
                    )
                  }
                  onToggle={(r) =>
                    void action(
                      () =>
                        put(`/resources/${r.id}`, {
                          ...r,
                          enabled: !r.enabled,
                        }),
                      "状态已下发",
                    ).catch(() => {})
                  }
                  busy={busy}
                />
              )}
              <div hidden={page !== "logs"}>
                <Logs agents={overview.agents} resources={resources} active={page === "logs"} />
              </div>
              <div hidden={page !== "traffic"}>
                <TrafficPage
                  agents={overview.agents}
                  resources={resources}
                  counters={counters}
                  active={page === "traffic"}
                />
              </div>
              {page === "settings" && (
                <SettingsPage
                  onChanged={setToast}
                  onLogout={() => setAuth(false)}
                />
              )}
            </>
          )}
        </main>
      </div>
      {toast && (
        <div className="toast" role="status">
          {toast}
        </div>
      )}
      {demoMode && demoFailures && (
        <Modal title="失败案例" wide size="details" onClose={() => setDemoFailures(null)}>
          <div className="table-scroll">
            <table className="demo-failures-table">
              <thead><tr><th>案例</th><th>原因</th><th>查看</th></tr></thead>
              <tbody>
                {demoFailures.map((item) => (
                  <tr key={item.title}>
                    <td>{item.title}</td>
                    <td className="error-text">{item.error}</td>
                    <td><button className="table-button" onClick={() => {
                      setDemoFailures(null);
                      window.location.hash = item.page;
                      if (item.resource) setEditor({ resource: item.resource,
                        isNew: !!item.isNew, initialError: item.isNew ? item.error : undefined });
                      else setDetail(item.agent_id);
                    }}>查看</button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Modal>
      )}
      {addAgent && (
        <AgentForm
          onClose={() => setAddAgent(false)}
          onCreated={(r) => {
            setAddAgent(false);
            setEnrollment(r);
            void refresh();
          }}
        />
      )}
      {enrollment && (
        <Modal
          title="安装子 agent · 仅首次需要"
          size="details"
          onClose={() => setEnrollment(null)}
          wide
        >
          <Enrollment
            data={enrollment}
            agent={overview.agents.find((a) => a.id === enrollment.agent_id)}
            onInstalled={refresh}
          />
        </Modal>
      )}
      {editor && (
        <ResourceEditor
          data={editor}
          agents={overview.agents}
          onClose={() => setEditor(null)}
          onSaved={async (r) => {
            await (editor.isNew
              ? post("/resources", r)
              : put(`/resources/${r.id}`, r));
            setEditor(null);
            const agent = overview.agents.find((a) => a.id === r.agent_id);
            setToast(
              demoMode
                ? "示例配置已应用"
                : agent && online(agent)
                  ? "配置已保存，正在自动下发"
                  : "配置已保存，服务器连接后自动应用",
            );
            await refresh();
          }}
        />
      )}
      {selectedAgent && (
        <Modal title={`${selectedAgent.name} · 服务器设置`} size="server-settings" wide onClose={() => setDetail(null)}>
          <div className="server-detail">
            <h3>服务器信息</h3>
            <div className="detail-summary">
              <Status agent={selectedAgent} showReason={false} />
              <AgentAddresses agent={selectedAgent} />
            </div>
            <AgentEdit agent={selectedAgent} onSaved={refresh}>
              <h3>核心</h3>
              <CoreManager
                key={selectedAgent.id}
                agent={selectedAgent}
                resources={resources}
                onSaved={refresh}
              />
              <ErrorText error={selectedStatus?.kind === "failed" ? selectedStatus.reason || "" : ""} />
            </AgentEdit>
          </div>
        </Modal>
      )}
      {confirm && (
        <Confirm
          {...confirm}
          onConfirm={confirm.action}
          onClose={() => setConfirm(null)}
        />
      )}
      {needsServer && (
        <Modal title="先选择一台服务器" size="compact" onClose={() => setNeedsServer(false)}>
          <p>
            当前没有可管理的服务器。到“服务器”页添加服务器并完成首次安装后，即可直接创建节点和转发
          </p>
          <div className="form-actions">
            <a className="button" href="/demo/#servers">
              <FlaskConical />
              查看示例
            </a>
            <button
              className="primary"
              onClick={() => {
                setNeedsServer(false);
                window.location.hash = "servers";
              }}
            >
              <HardDrive />
              前往服务器
            </button>
          </div>
        </Modal>
      )}
      {link && (
        <Modal title="节点连接" onClose={() => setLink(null)}>
          {demoMode && (
            <div className="notice">
              <FlaskConical />
              <span>
                这是示例连接，没有实际代理服务；示例密钥为公开测试数据
              </span>
            </div>
          )}
          <textarea {...nonCredentialAutofill} className="link-output" readOnly value={link} />
          <CopyButton value={link} />
        </Modal>
      )}

    </div>
  );
}

function Login({ onLogin }: { onLogin: (csrf: string) => void }) {
  const [password, setPassword] = useState(""),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const r = await post<{ csrf: string }>("/login", { password });
      onLogin(r.csrf);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="login-screen">
      <form id="login-form" className="login-panel" method="post" onSubmit={submit}>
        <span className="wordmark">
          <Brand />
        </span>
        <h1>登录控制台</h1>
        <Field label="管理员密码">
          <input
            id="login-password"
            name="password"
            autoFocus
            required
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="输入管理员密码"
          />
        </Field>
        <ErrorText error={error} />
        <Submit busy={busy}>登录</Submit>
        <a className="login-demo" href="/demo/#servers">
          <FlaskConical />
          先体验示例，无需登录
        </a>
      </form>
    </div>
  );
}
function Summary({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="summary-item">
      <div className="metric-label">{label}</div>
      <div className="metric-value">{value}</div>
    </div>
  );
}
function StateLabel({ state, showReason = true }: { state: DisplayStatus; showReason?: boolean }) {
  return <>
    <span className={`status ${state.kind === "enabled" ? "is-online" : state.kind === "failed" ? "is-error" : ""}`} title={state.reason}>
      <span />{statusLabels[state.kind]}
    </span>
    {showReason && state.reason && <span className={`cell-sub application-reason${state.kind === "failed" ? " is-error" : ""}`}
      title={state.reason} role={state.kind === "failed" ? "alert" : undefined}>{state.reason}</span>}
  </>;
}
function Status({ agent, showReason = true }: { agent: Agent; showReason?: boolean }) {
  return <StateLabel showReason={showReason} state={agentStatus(agent)} />;
}
function CoreBadge({
  children,
  running,
}: {
  children: ReactNode;
  running: boolean;
}) {
  return (
    <span className={running ? "core-badge running" : "core-badge"}>
      <span />
      {children}
    </span>
  );
}
function ServerTable({
  agents,
  resources,
  monthlyTraffic,
  onManage,
  onCreate,
  onToggle,
  onDelete,
  onForget,
  busy,
  onAdd,
}: {
  agents: Agent[];
  resources: Resource[];
  monthlyTraffic: Record<string, number>;
  onManage: (id: string) => void;
  onCreate: (kind: "vless" | "forward", id: string) => void;
  onToggle: (agent: Agent) => void;
  onDelete: (agent: Agent) => void;
  onForget: (agent: Agent) => void;
  busy: boolean;
  onAdd: () => void;
}) {
  const [q, setQ] = useState(""),
    [state, setState] = useState("all");
  const filtered = agents.filter(
    (a) =>
      `${a.name} ${a.address} ${a.ipv4 || ""} ${a.ipv6 || ""}`
        .toLowerCase()
        .includes(q.toLowerCase()) &&
      (state === "all" || state === agentStatus(a).kind),
  );
  return (
    <section className="panel server-panel">
      <div className="toolbar">
        <div className="search-field">
          <Search />
          <input {...nonCredentialAutofill}
            aria-label="搜索服务器"
            placeholder="搜索服务器名称或地址…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
        <Select
          aria-label="服务器状态"
          value={state}
          onChange={(e) => transitionContent(() => setState(e.target.value))}
        >
          <option value="all">全部状态</option>
          {Object.entries(statusLabels).map(([kind, label]) => <option key={kind} value={kind}>{label}</option>)}
        </Select>
      </div>
      {filtered.length ? (
        <>
        <p className="mobile-table-hint">左右滑动查看核心、流量与操作</p>
        <div className="table-scroll">
          <table className="server-table">
            <thead>
              <tr>
                <th>服务器 / 地址</th>
                <th>状态</th>
                <th>核心服务</th>
                <th>本月合计流量</th>
                <th className="action-column">操作</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((a) => {
                const assigned = resources.filter((r) => r.agent_id === a.id);
                const used = monthlyTraffic[a.id] || 0;
                return (
                  <tr key={a.id}>
                    <td>
                      <div className="server-name">
                        <ServerFlag country={a.country_code} />
                        <div>
                          <button
                            className="name-button"
                            onClick={() => onManage(a.id)}
                          >
                            {a.name}
                          </button>
                          <AgentAddresses agent={a} />
                        </div>
                      </div>
                    </td>
                    <td>
                      <Status agent={a} />
                    </td>
                    <td>
                      <div className="core-badges">
                        {a.cores
                          .filter((c) => c.state !== "missing")
                          .map((c) => (
                            <CoreBadge
                              key={c.core}
                              running={c.state === "running"}
                            >
                              {c.core === "realm" ? "Realm" : c.core}
                            </CoreBadge>
                          ))}
                        {!a.cores.some((c) => c.state !== "missing") && (
                          <span className="muted">未安装核心</span>
                        )}
                      </div>
                      <div className="cell-sub">
                        {
                          assigned.filter((r) => r.spec.type !== "forward")
                            .length
                        }{" "}
                        个节点 ·{" "}
                        {
                          assigned.filter((r) => r.spec.type === "forward")
                            .length
                        }{" "}
                        条转发
                      </div>
                    </td>
                    <td className="tabular">
                      <strong className="normal-weight">{bytes(used)}</strong>
                    </td>
                    <td className="action-column">
                      <ActionMenu label={a.name}>
                        <ActionMenuPower enabled={a.enabled !== false && !a.revoked} busy={busy} disabled={!online(a) || a.deleting} label="服务器" onToggle={() => onToggle(a)} />
                        <button role="menuitem" onClick={() => onManage(a.id)}>
                          <Settings /><span>服务器设置</span>
                        </button>
                        <PendingButton role="menuitem" busy={busy} disabled={a.revoked || a.deleting} onClick={() => onCreate("vless", a.id)}>
                          <InfinityIcon /><span>创建节点</span>
                        </PendingButton>
                        <PendingButton role="menuitem" busy={busy} disabled={a.revoked || a.deleting} onClick={() => onCreate("forward", a.id)}>
                          <ArrowRightLeft /><span>创建转发</span>
                        </PendingButton>
                        <hr />
                        <PendingButton role="menuitem" className="danger-text" busy={busy} disabled={(a.deleting && !a.error) || (a.enrolled && !a.uninstall_supported)} onClick={() => onDelete(a)}>
                          <Trash2 /><span>{a.deleting ? "重试清理" : "删除服务器"}</span>
                        </PendingButton>
                        {a.last_seen <= Date.now() - 60_000 && (
                          <PendingButton role="menuitem" className="danger-text" busy={busy} onClick={() => onForget(a)}>
                            <Trash2 /><span>本地已卸载，移除记录</span>
                          </PendingButton>
                        )}
                      </ActionMenu>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        </>
      ) : (
        <Empty
          title={agents.length ? "没有匹配的服务器" : "接入第一台服务器"}
          description={
            agents.length
              ? "调整搜索词或状态筛选"
              : "主服务器也可以接入，统一创建节点与转发规则"
          }
          action={
            !agents.length && (
              <button className="primary" onClick={onAdd}>
                添加服务器
              </button>
            )
          }
        />
      )}
    </section>
  );
}
function ResourceTable({
  resources,
  agents,
  counters,
  onCreate,
  onEdit,
  onCopy,
  onDelete,
  onToggle,
  onLink,
  busy,
}: {
  resources: Resource[];
  agents: Agent[];
  counters: Record<string, Counter>;
  onCreate: () => void;
  onEdit: (r: Resource) => void;
  onCopy: (r: Resource) => void;
  onDelete: (r: Resource) => void;
  onToggle: (r: Resource) => void;
  onLink: (r: Resource) => void;
  busy: boolean;
}) {
  const [q, setQ] = useState(""),
    [agent, setAgent] = useState("");
  const hasForwards = resources.some((r) => r.spec.type === "forward");
  const filtered = resources.filter(
    (r) =>
      (!agent || r.agent_id === agent) &&
      `${r.name} ${r.public_host} ${r.listen_port} ${r.spec.type === "forward" ? `${r.spec.remote_host} ${r.spec.remote_port}` : ""}`
        .toLowerCase()
        .includes(q.toLowerCase()),
  );
  return (
    <section className="panel">
      <div className="toolbar">
        <div className="search-field">
          <Search />
          <input {...nonCredentialAutofill}
            aria-label="搜索配置"
            placeholder="搜索名称、地址或端口…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
        <AgentSelect agents={agents} value={agent} onChange={(value) => transitionContent(() => setAgent(value))} />
      </div>
      {filtered.length ? (
        <div className="table-scroll">
          <table className={`resource-table${hasForwards ? " forward-table" : ""}`}>
            <thead>
              <tr>
                <th>配置 / 协议</th>
                <th>{hasForwards ? "中转" : "服务器 / 地址"}</th>
                {hasForwards && <th>落地</th>}
                <th>状态</th>
                <th>入站流量</th>
                <th>出站流量</th>
                <th>合计流量</th>
                <th className="action-column">操作</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((r) => {
                const spec = r.spec;
                const server = agents.find((a) => a.id === r.agent_id);
                const serverStopped = !server || server.enabled === false || server.revoked;
                const landing =
                  spec.type === "forward"
                    ? agents.find((a) => r.address_bindings?.target_agent_id
                        ? a.id === r.address_bindings.target_agent_id
                        : [primaryAddress(a), a.address, a.ipv4, a.ipv6].includes(spec.remote_host))
                    : undefined;
                const used =
                  (counters[r.id]?.upload || 0) +
                  (counters[r.id]?.download || 0);
                const status = resourceStatus(r, server, used);
                return (
                  <tr key={r.id}>
                    <td data-label="配置">
                      <div className="resource-name">
                        <ServerFlag country={(spec.type === "forward" ? landing : agents.find((a) => a.id === r.agent_id))?.country_code} />
                        <div>
                          <button className="name-button" title={r.name} onClick={() => onEdit(r)}>{r.name}</button>
                          <div className="cell-sub">{protocol(r)}</div>
                        </div>
                      </div>
                    </td>
                    <td data-label={hasForwards ? "中转" : "服务器 / 地址"}>
                      <span title={server?.name || "已删除"}>{server?.name || "已删除"}</span>
                      <code className="cell-sub" title={address(r.public_host || primaryAddress(server), r.listen_port)}>
                        {address(r.public_host || primaryAddress(server), r.listen_port)}
                      </code>
                    </td>
                    {r.spec.type === "forward" && <td data-label="落地">
                        <>
                          {landing && <div title={landing.name}>{landing.name}</div>}
                          <code className={landing ? "cell-sub" : ""} title={address(r.spec.remote_host, r.spec.remote_port)}>
                            {address(r.spec.remote_host, r.spec.remote_port)}
                          </code>
                        </>
                    </td>}
                    <td data-label="状态">
                      <StateLabel state={status} />
                    </td>
                    <td className="tabular" data-label="入站流量">
                      {bytes(counters[r.id]?.upload || 0)}
                    </td>
                    <td className="tabular" data-label="出站流量">
                      {bytes(counters[r.id]?.download || 0)}
                    </td>
                    <td className="tabular" data-label="合计流量">
                      <TrafficUsage used={used} quota={r.quota_bytes} label={r.name} />
                      <div className="cell-sub">{r.quota_cycle ? `${cycleLabel(r.quota_cycle)} · ${quotaDay(quotaWindow(r.quota_cycle)[1]).replaceAll("-", "/")} 重置` : "一次性"}</div>
                    </td>
                    <td className="action-column" data-label="操作">
                      <ActionMenu label={r.name}>
                        <ActionMenuPower enabled={r.enabled} busy={busy} startBlocked={serverStopped ? "请先启动所属服务器" : undefined} label={r.spec.type === "forward" ? "转发" : "节点"} onToggle={() => onToggle(r)} />
                        {r.spec.type !== "forward" && (
                          <button
                            role="menuitem"
                            onClick={() => onLink(r)}
                          >
                            <Link2 /><span>连接信息</span>
                          </button>
                        )}
                        <button
                          role="menuitem"
                          onClick={() => onEdit(r)}
                        >
                          <Pencil /><span>{r.spec.type === "forward" ? "编辑转发" : "编辑节点"}</span>
                        </button>
                        {r.spec.type === "forward" && <button role="menuitem" onClick={() => onCopy(r)}>
                          <Copy /><span>复制转发</span>
                        </button>}
                        <hr />
                        <PendingButton
                          role="menuitem"
                          className="danger-text"
                          busy={busy}
                          onClick={() => onDelete(r)}
                        >
                          <Trash2 /><span>{r.spec.type === "forward" ? "删除转发" : "删除节点"}</span>
                        </PendingButton>
                      </ActionMenu>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : (
        <Empty
          title={resources.length ? "没有匹配的配置" : "还没有配置"}
          description={
            resources.length
              ? "调整搜索词或服务器筛选"
              : "选择服务器创建配置，保存后自动下发"
          }
          action={
            !resources.length && (
              <button className="primary" onClick={onCreate}>
                创建配置
              </button>
            )
          }
        />
      )}
    </section>
  );
}
function AgentSelect({
  agents,
  value,
  onChange,
}: {
  agents: Agent[];
  value: string;
  onChange: (id: string) => void;
}) {
  return (
    <Select
      aria-label="选择服务器"
      value={value}
      onChange={(e) => onChange(e.target.value)}
    >
      <option value="">全部服务器</option>
      {agents.map((a) => (
        <option key={a.id} value={a.id}>
          {a.name}
        </option>
      ))}
    </Select>
  );
}
function AgentForm({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (r: {
    agent_id: string;
    token: string;
    expires_at: number;
    controller?: string;
  }) => void;
}) {
  const [name, setName] = useState(""),
    [cores, setCores] = useState<string[]>(["sing-box", "realm"]),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      onCreated(await post("/agents", { name, cores }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal title="添加服务器" size="add-server" onClose={onClose}>
      <form {...nonCredentialAutofill} onSubmit={submit}>
        <Field label="服务器名称">
          <input {...nonCredentialAutofill}
            autoFocus
            required
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="例如：香港 · NAT 01"
            maxLength={100}
          />
        </Field>
        <fieldset className="core-selection" disabled={busy}>
          <legend>安装核心</legend>
          <div>
            {(["sing-box", "realm"] as const).map((core) => (
              <label key={core} className="checkbox-label">
                <input {...nonCredentialAutofill}
                  type="checkbox"
                  checked={cores.includes(core)}
                  onChange={(e) => setCores((selected) => e.target.checked
                    ? [...selected, core]
                    : selected.filter((value) => value !== core))}
                />
                {core === "realm" ? "Realm" : core}
              </label>
            ))}
          </div>
        </fieldset>
        <ErrorText error={error} />
        <div className="form-actions">
          <button type="button" onClick={onClose}>
            取消
          </button>
          <Submit busy={busy}>添加服务器</Submit>
        </div>
      </form>
    </Modal>
  );
}
function Enrollment({
  data,
  agent,
  onInstalled,
}: {
  data: {
    agent_id: string;
    token: string;
    expires_at: number;
    controller?: string;
  };
  agent?: Agent;
  onInstalled: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const origin = demoMode
    ? "https://hproxy.example.com"
    : data.controller || window.location.origin;
  const quote = (s: string) => `'${s.replace(/'/g, "'\\''")}'`;
  const command = `wget -qO- ${quote(`${origin}/install.sh`)} | sh -s -- -e ${quote(origin)} -t ${quote(data.token)}`;
  const connected = agent && online(agent);
  const issue = agent ? applicationIssue(agent) : null;
  return (
    <>
      {connected ? (
        <div className="setup-notice">
          <Check />
          <div>
            <strong>服务器已连接</strong>
          </div>
        </div>
      ) : (
        <div className="notice">
          <Info />
          <span>在服务器执行下方命令，安装完成后会自动出现在面板中</span>
        </div>
      )}
      {!connected && (
        <>
          <pre className="command-output">{command}</pre>
          <div className="install-actions">
            <CopyButton value={command} />
            {demoMode && (
              <PendingButton
                busy={busy}
                onClick={async () => {
                  setBusy(true);
                  try {
                    await post(`/agents/${data.agent_id}/simulate-install`);
                    await onInstalled();
                  } catch (e) {
                    setError((e as Error).message);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                模拟安装完成
              </PendingButton>
            )}
          </div>
          {!demoMode && window.location.protocol === "http:" && (
            <p className="muted">
              当前为本机预览；部署 HTTPS 主控后，可在其他服务器执行安装命令
            </p>
          )}
        </>
      )}
      <ErrorText error={error || (issue?.kind === "failed" ? issue.reason : "") || ""} />
    </>
  );
}
function AgentAddresses({ agent }: { agent: Agent }) {
  const v4 =
      agent.ipv4 ||
      (!agent.address.includes(":") &&
      /^\d+\.\d+\.\d+\.\d+$/.test(agent.address)
        ? agent.address
        : ""),
    v6 = agent.ipv6 || (agent.address.includes(":") ? agent.address : "");
  return (
    <div className="agent-addresses">
      {v4 && (
        <div>
          <span>IPv4</span>
          <code title={v4}>{v4}</code>
        </div>
      )}
      {v6 && (
        <div>
          <span>IPv6</span>
          <code title={v6}>{v6}</code>
        </div>
      )}
      {!v4 && !v6 && <span className="muted">等待上报地址</span>}
    </div>
  );
}
function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false),
    [error, setError] = useState("");
  return (
    <>
      <button
        className="primary copy-button"
        onClick={() => {
          navigator.clipboard
            .writeText(value)
            .then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 2000);
            })
            .catch(() =>
              setError("浏览器未允许复制，请在文本框中手动选择复制"),
            );
        }}
      >
        {copied ? "已复制" : "复制"}
      </button>
      <ErrorText error={error} />
    </>
  );
}
function AgentEdit({
  agent,
  onSaved,
  children,
}: {
  agent: Agent;
  onSaved: () => Promise<void>;
  children: ReactNode;
}) {
  const [name, setName] = useState(agent.name),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      await put(`/agents/${agent.id}`, { name, address: agent.address });
      await onSaved();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <form {...nonCredentialAutofill} onSubmit={submit}>
      <div className="form-grid">
        <Field label="名称" full>
          <input {...nonCredentialAutofill}
            required
            disabled={agent.deleting}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
      </div>
      {children}
      <ErrorText error={error} />
      <div className="form-actions">
        <Submit busy={busy} disabled={agent.deleting} />
      </div>
    </form>
  );
}
