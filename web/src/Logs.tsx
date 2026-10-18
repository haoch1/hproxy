import Select from "./Select";
import { useEffect, useState } from "react";
import { Search } from "lucide-react";
import { api, demoMode } from "./api";
import type { Agent, Log, Resource } from "./types";
import { bytes, time } from "./types";
import { Empty, ErrorText, Modal } from "./components";
import LogTimePicker, { timeBounds } from "./LogTimePicker";
import type { LogTimeRange } from "./LogTimePicker";
import LogPagination from "./LogPagination";
import { transitionContent } from "./contentTransition";
import { nonCredentialAutofill } from "./formAutofill";
import { liveReload, subscribeLogUpdates } from "./logUpdates";

type LogPage = { rows: Log[]; total: number; pages: number; page: number };

function targetAddress(log: Log) {
  if (!log.domain) return log.destination || "—";
  const port = log.destination.match(/^(?:\[[^\]]+\]|[^:]+):(\d{1,5})$/)?.[1];
  return port ? `${log.domain}:${port}` : log.domain;
}

function resultText(log: Log) {
  if (log.result.startsWith("error")) {
    const reason = log.result.replace(/^error\s*:?\s*/, "").trim();
    return reason ? `失败：${reason}` : "失败";
  }
  return log.closed_at ? "已结束" : "连接中";
}

export default function Logs({
  agents,
  resources,
  active,
}: {
  agents: Agent[];
  resources: Resource[];
  active: boolean;
}) {
  const [q, setQ] = useState(""),
    [searchQuery, setSearchQuery] = useState(""),
    [agent, setAgent] = useState(""),
    [range, setRange] = useState<LogTimeRange>({ kind: "month" }),
    [page, setPage] = useState(1),
    [pageSize, setPageSize] = useState(20);
  const [rows, setRows] = useState<Log[]>([]),
    [total, setTotal] = useState(0),
    [pages, setPages] = useState(1),
    [busy, setBusy] = useState(true),
    [loaded, setLoaded] = useState(false),
    [error, setError] = useState(""),
    [detail, setDetail] = useState<Log | null>(null);
  useEffect(() => {
    const timer = setTimeout(() => { setSearchQuery(q); setPage(1); }, 250);
    return () => clearTimeout(timer);
  }, [q]);
  useEffect(() => {
    if (!active) return;
    setBusy(true);
    const loader = liveReload(() => {
      // Recompute moving bounds per request, including catch-up after reconnect.
      // A fixed day's identical query must still reload when its logs change.
      const { from, to } = timeBounds(range);
      const params = new URLSearchParams({
        q: searchQuery, page: String(page), page_size: String(pageSize),
        from: String(from), to: String(to),
      });
      if (agent) params.set("agent_id", agent);
      return api<LogPage>(`/logs?${params}`);
    }, (r) => {
      transitionContent(() => {
        setRows(r.rows);
        setTotal(r.total);
        setPages(r.pages);
        setPage(r.page);
        setDetail((current) => current && (r.rows.find((row) => row.agent_id === current.agent_id && row.id === current.id) ?? current));
        setError("");
        setBusy(false);
        setLoaded(true);
      });
    }, (e) => {
      setError(e instanceof Error ? e.message : "加载连接日志失败");
      setBusy(false);
      setLoaded(true);
    });
    loader.reload();
    // Demo samples stay local; production receives committed changes over SSE.
    const demoTimer = demoMode ? setInterval(loader.reload, 10000) : undefined;
    const unsubscribe = demoMode ? undefined : subscribeLogUpdates(loader.reload);
    return () => {
      loader.stop();
      unsubscribe?.();
      clearInterval(demoTimer);
    };
  }, [active, searchQuery, agent, range, page, pageSize]);
  return (
    <>
      <section className="panel logs-panel">
        <div className="toolbar wrap logs-toolbar">
          <div className="search-field">
            <Search />
            <input {...nonCredentialAutofill}
              aria-label="搜索来源或目标"
              placeholder="搜索地址或域名…"
              value={q}
              onChange={(e) => {
                setQ(e.target.value);
              }}
            />
          </div>
          <Select
            aria-label="选择服务器"
            value={agent}
            onChange={(e) => {
              setAgent(e.target.value);
              setPage(1);
            }}
          >
            <option value="">全部服务器</option>
            {agents.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </Select>
          <LogTimePicker
            value={range}
            onChange={(value) => {
              setRange(value);
              setPage(1);
            }}
          />
        </div>
        <ErrorText error={error} />
        {rows.length || !loaded ? (
          <>
          <p className="mobile-table-hint">左右滑动查看完整日志</p>
          <div className="table-scroll" aria-busy={busy}>
            <table className="log-table">
              <colgroup>
                {[12, 17, 20, 16, 9, 10, 9, 7].map((width, index) => (
                  <col key={index} style={{ width: `${width}%` }} />
                ))}
              </colgroup>
              <thead>
                <tr>
                  <th>开始时间</th>
                  <th>来源</th>
                  <th>地址</th>
                  <th>节点</th>
                  <th>入站流量</th>
                  <th>出站流量</th>
                  <th>合计流量</th>
                  <th>状态</th>
                </tr>
              </thead>
              <tbody>
                {!loaded && Array.from({ length: pageSize }, (_, index) => (
                  <tr key={`placeholder-${index}`} aria-hidden="true" className="log-placeholder">
                    {Array.from({ length: 8 }, (_, column) => <td key={column}><span className="skeleton" /></td>)}
                  </tr>
                ))}
                {rows.map((r) => (
                  <tr
                    key={`${r.agent_id}:${r.id}`}
                    onClick={() => setDetail(r)}
                    className="clickable"
                  >
                    <td className="tabular">{time(r.started_at)}</td>
                    <td className="tabular" title={r.source}>{r.source}</td>
                    <td>
                      <button
                        className="name-button log-target"
                        title={`${targetAddress(r)}${r.domain && r.destination ? `\n${r.destination}` : ""}`}
                        onClick={() => setDetail(r)}
                      >
                        {targetAddress(r)}
                      </button>
                    </td>
                    <td>
                      {resources.find((n) => n.id === r.resource_id)?.name ||
                        "已删除节点"}
                    </td>
                    <td className="tabular">{bytes(r.upload)}</td>
                    <td className="tabular">{bytes(r.download)}</td>
                    <td className="tabular">{bytes(r.upload + r.download)}</td>
                    <td>
                      <span
                        className={
                          r.result.startsWith("error")
                            ? "error-text"
                            : r.closed_at
                              ? "muted"
                              : "success-text"
                        }
                      >
                        {r.result.startsWith("error")
                          ? "失败"
                          : r.closed_at
                            ? "已结束"
                            : "连接中"}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          </>
        ) : (
          <Empty title="没有连接记录" />
        )}
        <LogPagination
          page={page}
          pages={pages}
          total={total}
          pageSize={pageSize}
          busy={busy}
          ready={loaded}
          onPage={(value) => { setBusy(true); setPage(value); }}
          onPageSize={(size) => {
            setBusy(true);
            setPageSize(size);
            setPage(1);
          }}
        />
      </section>
      {detail && (
        <Modal title="连接详情" size="details" onClose={() => setDetail(null)}>
          <dl className="detail-grid">
            {[
              [
                "服务器",
                agents.find((a) => a.id === detail.agent_id)?.name ||
                  "已删除服务器",
              ],
              [
                "节点",
                resources.find((r) => r.id === detail.resource_id)?.name ||
                  "已删除节点",
              ],
              ["开始时间", time(detail.started_at)],
              ["结束时间", time(detail.closed_at)],
              ["来源", detail.source],
              ["地址", targetAddress(detail)],
              ["解析地址", detail.destination],
              ["网络协议", detail.network.toUpperCase()],
              ["入站流量", bytes(detail.upload)],
              ["出站流量", bytes(detail.download)],
              ["合计流量", bytes(detail.upload + detail.download)],
              ["结果", resultText(detail)],
            ].map(([label, value]) => (
              <div key={label}>
                <dt>{label}</dt>
                <dd>{value}</dd>
              </div>
            ))}
          </dl>
        </Modal>
      )}
    </>
  );
}
