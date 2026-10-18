import { useEffect, useState } from "react";
import { api } from "./api";
import type { Agent, Counter, Resource, Traffic } from "./types";
import { bytes } from "./types";
import { ErrorText } from "./components";
import Select from "./Select";
import TrafficChart from "./TrafficChart";
import { transitionContent } from "./contentTransition";
import { buckets, rangeStart } from "./traffic";
import type { TrafficRange } from "./traffic";

export default function TrafficPage({
  agents,
  resources,
  counters,
  active: visible,
}: {
  agents: Agent[];
  resources: Resource[];
  counters: Record<string, Counter>;
  active: boolean;
}) {
  const [agent, setAgent] = useState(""),
    [range, setRange] = useState<TrafficRange>("month"),
    [result, setResult] = useState<{ rows: Traffic[]; range: TrafficRange; agent: string; ready: boolean }>({ rows: [], range: "month", agent: "", ready: false }),
    [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let active = true, pending: Promise<void> | null = null;
    const load = (silent = false): Promise<void> => {
      if (pending) return pending;
      if (!silent) setLoading(true);
      const q = new URLSearchParams({ from: String(rangeStart(range)) });
      if (agent) q.set("agent_id", agent);
      pending = api<{ rows: Traffic[] }>(`/traffic?${q}`)
        .then(r => { if (active) {
          const commit = () => { if (active) { setResult({ rows: r.rows, range, agent, ready: true }); setError(""); } };
          if (silent) commit(); else transitionContent(commit);
        } })
        .catch(e => { if (active) setError(e.message); })
        .finally(() => { pending = null; if (active) setLoading(false); });
      return pending;
    };
    void load();
    const timer = visible ? setInterval(() => void load(true), 10000) : undefined;
    return () => { active = false; clearInterval(timer); };
  }, [agent, range, visible]);
  const filtered = resources.filter(
    (r) => (!result.agent || r.agent_id === result.agent) && r.spec.type !== "forward",
  );
  const points = buckets(result.rows, result.range),
    date = (at: number) =>
      new Intl.DateTimeFormat("zh-CN", {
        timeZone: "Asia/Shanghai",
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
      }).format(at);
  return (
    <div className="traffic-content">
      <div className="traffic-toolbar toolbar">
        <div className="traffic-periods" aria-label="时间范围">
          {(
            [
              ["month", "本月"],
              [7, "近 7 天"],
              [30, "近 30 天"],
            ] as const
          ).map(([n, label]) => (
            <button
              key={n}
              className={"button" + (range === n ? " primary" : "")}
              aria-pressed={range === n}
              onClick={() => setRange(n)}
            >
              {label}
            </button>
          ))}
        </div>
        <div className="traffic-instance-picker">
          <Select
            aria-label="流量服务器"
            value={agent}
            onChange={(e) => setAgent(e.target.value)}
          >
            <option value="">全部服务器</option>
            {agents.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </Select>
        </div>
      </div>
      <ErrorText error={error} />
      <div className="traffic-overview" aria-busy={loading}>
        {([['入站流量', 'upload'], ['出站流量', 'download'], ['合计流量', 'total']] as const).map(([label, key]) =>
          <div key={key}><span>{label}</span><strong>{bytes(points.reduce((sum, p) => sum + (key === 'total' ? p.upload + p.download : p[key]), 0))}</strong></div>)}
      </div>
      <p className="traffic-range">{date(points[0].at)} 至 {date(points[points.length - 1].at)}（UTC+08:00）</p>
      <TrafficChart rows={points.map(p => ({date: date(p.at).replaceAll('/', '-'), inBytes: p.upload, outBytes: p.download}))} loading={loading && !result.ready} />
      <section className="traffic-details traffic-section" aria-labelledby="node-traffic-title">
        <div className="panel-header"><h2 id="node-traffic-title">节点累计流量</h2></div>
        <div className="table-wrap table-scroll">
          <table className="traffic-data-table traffic-daily-table">
            <colgroup><col style={{ width: "40%" }} /><col style={{ width: "20%" }} /><col style={{ width: "20%" }} /><col style={{ width: "20%" }} /></colgroup>
            <thead>
              <tr>
                <th>节点 / 服务器</th>
                <th>入站流量</th>
                <th>出站流量</th>
                <th>合计流量</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((r) => {
                const c = counters[r.id] || { upload: 0, download: 0 },
                  total = c.upload + c.download;
                return (
                  <tr key={r.id}>
                    <td data-label="节点">
                      <strong>{r.name}</strong>
                      <div className="cell-sub">
                        {agents.find((a) => a.id === r.agent_id)?.name}
                      </div>
                    </td>
                    <td data-label="入站流量" className="tabular">{bytes(c.upload)}</td>
                    <td data-label="出站流量" className="tabular">{bytes(c.download)}</td>
                    <td data-label="合计流量" className="tabular">{bytes(total)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </section>
      <section className="traffic-daily traffic-section" aria-labelledby="daily-traffic-title">
        <div className="panel-header"><h2 id="daily-traffic-title">每日明细</h2></div>
        <div className="table-wrap table-scroll">
          <table className="traffic-data-table traffic-daily-table">
            <colgroup><col style={{ width: "40%" }} /><col style={{ width: "20%" }} /><col style={{ width: "20%" }} /><col style={{ width: "20%" }} /></colgroup>
            <thead>
              <tr>
                <th>日期</th>
                <th>入站流量</th>
                <th>出站流量</th>
                <th>合计流量</th>
              </tr>
            </thead>
            <tbody>
              {[...points].reverse().map((p) => (
                <tr key={p.at}>
                  <td data-label="日期">{date(p.at)}</td>
                  <td data-label="入站流量" className="tabular">{bytes(p.upload)}</td>
                  <td data-label="出站流量" className="tabular">{bytes(p.download)}</td>
                  <td data-label="合计流量" className="tabular">{bytes(p.upload + p.download)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
}
