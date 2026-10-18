import Select from "./Select";
import ConfigViewer from "./ConfigViewer";
import { ArrowRight } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, post, put } from "./api";
import { Confirm, ErrorText, Field, Modal, PendingButton, Submit } from "./components";

interface Versions {
  phase: string;
  error: string | null;
  source_configured: boolean;
  can_update: boolean;
  checked_at: number | null;
  build: string | null;
  components: {
    key: string;
    name: string;
    current: string;
    latest: string | null;
    state: string;
    can_update?: boolean;
    blocked_reason?: string;
  }[];
  agents: { name: string; state: string; error: string; online: boolean }[];
}
let cachedVersions: Versions | null = null;
const working = (phase: string) =>
  ["checking", "downloading", "restarting", "verifying"].includes(phase);
const states: Record<string, string> = {
  current: "最新",
  newer: "无需更新",
  unchecked: "待检查",
};
const retentionDays = [7, 30, 90];
function versionState(component: Versions["components"][number]) {
  if (!component.latest || component.state === "unchecked") return "unchecked";
  const normalize = (version: string) => version.trim().replace(/^v/, "");
  return normalize(component.current) === normalize(component.latest) ? "current" : component.state;
}
const hasNewVersion = (component: Versions["components"][number]) => versionState(component) === "available";
export default function SettingsPage({
  onChanged,
  onLogout,
}: {
  onChanged: (message: string) => void;
  onLogout: () => void;
}) {
  const [days, setDays] = useState(7),
    [savedDays, setSavedDays] = useState(7),
    [loaded, setLoaded] = useState(false);
  const [error, setError] = useState(""),
    [saving, setSaving] = useState(false);
  const [versions, setVersions] = useState<Versions | null>(cachedVersions);
  const [requesting, setRequesting] = useState(false),
    [awaiting, setAwaiting] = useState(false);
  const [passwordOpen, setPasswordOpen] = useState(false),
    [configOpen, setConfigOpen] = useState(false),
    [confirm, setConfirm] = useState<"all" | Versions["components"][number] | null>(null);
  const pending = useRef<{ key: string; version: string; name: string }[]>([]);
  const versionRequest = useRef(0);
  useEffect(() => {
    api<{ log_days: number }>("/settings")
      .then((s) => {
        setDays(s.log_days);
        setSavedDays(s.log_days);
        setLoaded(true);
      })
      .catch((e) => setError(e.message));
  }, []);
  const loadVersions = useCallback(async () => {
    const request = ++versionRequest.current;
    try {
      const next = await api<Versions>("/updates");
      if (request !== versionRequest.current) return;
      cachedVersions = next;
      setVersions((previous) => JSON.stringify(previous) === JSON.stringify(next) ? previous : next);
      if (
        awaiting &&
        pending.current.length > 0 &&
        pending.current.every((item) => next.components.some((c) => c.key === item.key && c.current === item.version)) &&
        !working(next.phase)
      ) {
        setAwaiting(false);
        onChanged(`${pending.current.map((c) => c.name).join("、")}更新已发布，服务器正在同步`);
        pending.current = [];
      } else if (next.phase === "failed") setAwaiting(false);
    } catch (e) {
      if (request === versionRequest.current && !awaiting) setError((e as Error).message);
    }
  }, [awaiting, onChanged]);
  // Polling also reconnects after the controller replaces its running binary.
  useEffect(() => {
    void loadVersions();
    window.addEventListener("updates-changed", loadVersions);
    const timer = window.setInterval(
      loadVersions,
      awaiting || working(versions?.phase || "") ? 2000 : 10000,
    );
    return () => {
      clearInterval(timer);
      window.removeEventListener("updates-changed", loadVersions);
    };
  }, [loadVersions, awaiting, versions?.phase]);
  async function save(e: FormEvent) {
    e.preventDefault();
    if (saving || !loaded) return;
    setSaving(true);
    setError("");
    try {
      await put("/settings", { log_days: days });
      setSavedDays(days);
      onChanged("日志保留设置已保存");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSaving(false);
    }
  }
  async function check() {
    setRequesting(true);
    setError("");
    try {
      await post("/updates/check");
      await loadVersions();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setRequesting(false);
    }
  }
  async function install() {
    const component = confirm === "all" ? "all" : confirm?.key;
    if (!component) return;
    pending.current = (versions?.components || [])
      .filter((c) => hasNewVersion(c) && (component === "all" || c.key === component))
      .map((c) => ({ key: c.key, name: c.name, version: c.latest! }));
    if (!pending.current.length) throw new Error("所选组件已是最新版本");
    await post("/updates/apply", { build: versions?.build, component });
    setAwaiting(true);
    await loadVersions();
  }
  const busy = requesting || awaiting || working(versions?.phase || "");
  const available = versions?.components.some(hasNewVersion);
  const updateMessage =
    !versions ? "" : versions.phase === "downloading"
        ? "正在下载并校验…"
        : versions?.phase === "verifying"
          ? "正在确认更新结果…"
          : awaiting || versions?.phase === "restarting"
            ? "正在重启主控…"
            : !versions?.source_configured
              ? "尚未配置更新源"
              : !versions?.can_update
                ? "在线更新需使用主控安装脚本部署"
                : "";
  const failures = versions?.agents.filter((a) => a.state === "failed") || [];
  return (
    <div className="settings-layout">
      <ErrorText error={error} />
      <section className="panel settings-general">
        <div className="panel-header">
          <h2>常规设置</h2>
        </div>
        <form className="setting-row" onSubmit={save}>
          <div className="setting-description">
            <label htmlFor="log-days">连接日志保留天数</label>
            <p id="log-retention-note">超期日志会自动删除，流量累计统计保留</p>
          </div>
          <div className="setting-control">
            <div className="setting-retention">
              <Select
                id="log-days"
                name="log-retention-days"
                aria-label="连接日志保留天数"
                aria-describedby="log-retention-note"
                required
                value={days}
                onChange={(e) => setDays(Number(e.target.value))}
                disabled={!loaded}
              >
                {[...new Set([...retentionDays, savedDays])].sort((a, b) => a - b).map((day) => (
                  <option key={day} value={day}>{day} 天</option>
                ))}
              </Select>
            </div>
            <Submit busy={saving} disabled={!loaded}>保存</Submit>
          </div>
        </form>
        <div className="setting-row">
          <span>登录密码</span>
          <button className="settings-action" onClick={() => setPasswordOpen(true)}>修改密码</button>
        </div>
        <div className="setting-row">
          <div className="setting-description">
            <span>运行配置</span>
            <p>查看服务器的 sing-box 与 Realm 配置文件</p>
          </div>
          <button className="settings-action" onClick={() => setConfigOpen(true)}>查看配置</button>
        </div>
      </section>
      <section className="panel settings-update">
        <div className="panel-header">
          <h2>版本更新</h2>
          <PendingButton
            className="settings-action"
            onClick={check}
            busy={busy}
            disabled={!versions?.source_configured}
          >
            检查更新
          </PendingButton>
        </div>
        <div className="version-table" role="table" aria-label="组件版本">
          <div className="version-row version-head" role="row">
            <span role="columnheader">组件</span>
            <span role="columnheader">当前版本</span>
            <span role="columnheader">最新版本</span>
            <span role="columnheader" className="version-control-heading">状态</span>
          </div>
          {(
            versions?.components ||
            ["hproxy", "sing-box", "Realm"].map<Versions["components"][number]>((name) => ({
              key: name,
              name,
              current: "—",
              latest: null,
              state: "unchecked",
            }))
          ).map((c) => {
            const state = versionState(c);
            return (
            <div className="version-row" role="row" key={c.key}>
              <strong role="cell">{c.name}</strong>
              <span role="cell" className="version-number">
                {c.current}
              </span>
              <span role="cell" className="version-number">
                {c.latest || "—"}
              </span>
              <span role="cell" className="version-control">
                {state === "available" ? (
                  <PendingButton className="component-update settings-action" aria-label={`更新 ${c.name}`}
                    title={c.blocked_reason || `仅更新 ${c.name}`}
                    busy={busy}
                    disabled={!versions?.can_update || !versions?.source_configured || c.can_update === false}
                    onClick={() => setConfirm(c)}>更新</PendingButton>
                ) : (
                  <span className={`version-state ${state}`}>
                    {states[state] || state}
                  </span>
                )}
              </span>
            </div>
          ); })}
        </div>
        <ErrorText error={versions?.error || ""} />
        {(available || updateMessage) && <div className="version-actions">
          <span role="status">
            {updateMessage}
          </span>
          {available && <PendingButton
            className="primary settings-action"
            onClick={() => setConfirm("all")}
            busy={busy}
            disabled={
              !available ||
              !versions?.can_update ||
              !versions?.source_configured
            }
          >
            更新全部
          </PendingButton>}
        </div>}
        {failures.length > 0 && (
          <details className="update-errors">
            <summary>{failures.length} 台服务器更新失败</summary>
            {failures.map((a) => (
              <p key={a.name}>
                {a.name}：{a.error || "请检查服务器连接"}
              </p>
            ))}
          </details>
        )}
      </section>
      {passwordOpen && (
        <Modal title="修改密码" onClose={() => setPasswordOpen(false)}>
          <PasswordForm
            onLogout={onLogout}
            onClose={() => setPasswordOpen(false)}
          />
        </Modal>
      )}
      {configOpen && (
        <Modal title="运行配置" wide fixed size="config-viewer" onClose={() => setConfigOpen(false)}>
          <ConfigViewer />
        </Modal>
      )}
      {confirm && (
        <Confirm
          size="update-confirm"
          confirmLabel="更新"
          title={confirm === "all" ? "更新全部组件" : `更新 ${confirm.name}`}
          description={confirm === "all"
            ? "仅更新有新版本的组件，服务会短暂重启"
            : <><span className="update-version"><span>{confirm.current}</span><ArrowRight aria-hidden="true" /><span>{confirm.latest}</span></span><span className="update-notice">更新时服务会短暂重启</span></>}
          onConfirm={install}
          onClose={() => setConfirm(null)}
        />
      )}
    </div>
  );
}
function PasswordForm({
  onLogout,
  onClose,
}: {
  onLogout: () => void;
  onClose: () => void;
}) {
  const [current, setCurrent] = useState(""),
    [password, setPassword] = useState(""),
    [repeat, setRepeat] = useState("");
  const [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  async function change(e: FormEvent) {
    e.preventDefault();
    if (password !== repeat) {
      setError("两次输入的新密码不一致");
      return;
    }
    setBusy(true);
    setError("");
    try {
      await put("/password", { current, password });
      onLogout();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <form id="password-change-form" method="post" onSubmit={change}>
      <ErrorText error={error} />
      <div className="form-grid">
        <Field label="当前密码" full>
          <input
            id="current-password"
            name="current-password"
            required
            type="password"
            autoComplete="current-password"
            value={current}
            onChange={(e) => setCurrent(e.target.value)}
          />
        </Field>
        <Field label="新密码（至少 12 字符）" full>
          <input
            id="new-password"
            name="new-password"
            required
            type="password"
            minLength={12}
            autoComplete="new-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </Field>
        <Field label="确认新密码" full>
          <input
            id="confirm-password"
            name="confirm-password"
            required
            type="password"
            minLength={12}
            autoComplete="new-password"
            value={repeat}
            onChange={(e) => setRepeat(e.target.value)}
          />
        </Field>
      </div>
      <div className="form-actions">
        <button type="button" onClick={onClose}>
          取消
        </button>
        <Submit busy={busy}>修改密码</Submit>
      </div>
    </form>
  );
}
