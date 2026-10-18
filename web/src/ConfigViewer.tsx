import { useEffect, useState } from "react";
import { Check, Copy } from "lucide-react";
import { api } from "./api";
import { Empty, ErrorText } from "./components";
import { RefreshButton } from "./RefreshButton";
import Select from "./Select";
import { byName, online } from "./types";
import type { Agent } from "./types";

type ConfigFile = { filename: string; content: Record<string, unknown> };
type Result = { key: string; data?: ConfigFile; error?: string };
export default function ConfigViewer() {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [agentId, setAgentId] = useState("");
  const [core, setCore] = useState("sing-box");
  const [loaded, setLoaded] = useState(false);
  const [listError, setListError] = useState("");
  const [results, setResults] = useState<Record<string, Result>>({});
  const [busy, setBusy] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState("");
  const key = `${agentId}/${core}`;
  const current = results[key];
  useEffect(() => {
    let cancelled = false;
    api<Agent[]>("/agents").then((items) => {
      if (cancelled) return;
      const sorted = byName(items);
      setAgents(sorted);
      setAgentId((sorted.find(online) || sorted[0])?.id || "");
      setLoaded(true);
    }).catch((e) => { if (!cancelled) { setListError(e.message); setLoaded(true); } });
    return () => { cancelled = true; };
  }, []);
  useEffect(() => {
    if (!agentId) return;
    let cancelled = false;
    setBusy(true);
    setCopyError("");
    api<ConfigFile>(`/agents/${agentId}/configs/${core}`).then((data) => {
      if (!cancelled) setResults((previous) => ({ ...previous, [key]: { key, data } }));
    }).catch((e) => { if (!cancelled) setResults((previous) => ({ ...previous,
      [key]: { key, data: previous[key]?.data, error: e.message } })); })
      .finally(() => { if (!cancelled) setBusy(false); });
    return () => { cancelled = true; };
  }, [agentId, core, refresh, key]);
  const text = current?.data ? JSON.stringify(current.data.content, null, 2) : "";
  useEffect(() => { setCopied(false); }, [text]);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(timer);
  }, [copied]);
  async function copy() {
    try { await navigator.clipboard.writeText(text); setCopied(true); setCopyError(""); }
    catch { setCopyError("复制失败，请选中配置内容后复制"); }
  }
  return (
    <div className="config-viewer">
      <div className="config-toolbar">
        <Select aria-label="配置所属服务器" value={agentId} disabled={!agents.length} onChange={(e) => setAgentId(e.target.value)}>
          {!agents.length && <option value="">选择服务器</option>}
          {agents.map((agent) => <option key={agent.id} value={agent.id}>{agent.name}</option>)}
        </Select>
        <div className="segmented" role="group" aria-label="配置核心">
          {[['sing-box', 'sing-box'], ['realm', 'Realm']].map(([value, name]) => (
            <button type="button" key={value} className={core === value ? "selected" : ""}
              aria-pressed={core === value} onClick={() => setCore(value)}>{name}</button>
          ))}
        </div>
      </div>
      <div className="config-file-head">
        <span>{core}.json</span>
        <div className="config-file-actions">
          <button type="button" className="icon-button" aria-label={copied ? "配置已复制" : "复制当前配置"}
            title={copied ? "已复制" : "复制当前内容"} disabled={!text} onClick={copy}>{copied ? <Check /> : <Copy />}</button>
          <RefreshButton className="icon-button" title="重新读取配置" text="" loading={busy}
            disabled={!agentId} onClick={() => setRefresh((value) => value + 1)} />
        </div>
      </div>
      <ErrorText error={listError || current?.error || copyError} />
      {text ? <pre className="config-code" tabIndex={0} aria-label="配置文件内容" aria-busy={busy}><code>{text}</code></pre>
        : <div className="config-empty">{!loaded || busy ? <span className="muted" role="status">正在读取配置…</span>
          : <Empty title={agents.length ? "暂时无法读取配置" : "暂无服务器"} />}</div>}
    </div>
  );
}
