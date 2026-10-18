import { useState } from "react";
import { put } from "./api";
import { ErrorText, PendingButton } from "./components";
import type { Agent, Resource } from "./types";
import { online } from "./types";

export default function CoreManager({
  agent,
  resources,
  onSaved,
}: {
  agent: Agent;
  resources: Resource[];
  onSaved: () => Promise<void>;
}) {
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const connected = online(agent);
  const waiting = agent.deleting || (agent.revision !== agent.applied && !agent.error);
  async function change(core: string, installed: boolean) {
    if (!online(agent) || busy || waiting) return;
    setBusy(core);
    try {
      await put(`/agents/${agent.id}/cores/${core}`, { installed });
      await onSaved();
      setError("");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy("");
    }
  }
  return (
    <>
      <div className="core-manager">
        {(["sing-box", "realm"] as const).map((core) => {
          const status = agent.cores.find((c) => c.core === core);
          const installed = !!status && status.state !== "missing";
          const used = resources.some(
            (r) =>
              r.agent_id === agent.id &&
              (r.spec.type === "forward") === (core === "realm"),
          );
          const requested = agent.core_policy?.includes(core) ?? installed;
          const pending = connected && waiting && requested !== installed;
          return (
            <div key={core} className="core-manager-row">
              <div className="core-manager-name">
                <strong>{core === "realm" ? "Realm" : core}</strong>
                <span>{core === "realm" ? "端口转发" : "代理节点"}</span>
              </div>
              <span
                className={
                  status?.state === "running" ? "success-text" : "muted"
                }
              >
                {pending
                    ? requested
                      ? "安装中…"
                      : "卸载中…"
                    : !installed
                      ? "未安装"
                      : status.state === "running"
                        ? "运行中"
                        : status.state === "error"
                          ? "异常"
                          : "已安装"}
              </span>
              <PendingButton
                type="button"
                busy={!!busy || !!waiting}
                disabled={
                  !agent.core_management ||
                  !connected ||
                  (installed && used)
                }
                title={
                  !connected
                    ? "服务器离线，连接后可管理核心"
                    : installed && used
                    ? "请先删除使用此核心的节点或转发"
                    : undefined
                }
                aria-label={`${installed ? "卸载" : "安装"} ${core === "realm" ? "Realm" : core}`}
                onClick={() => void change(core, !installed)}
              >
                {installed ? "卸载" : "安装"}
              </PendingButton>
            </div>
          );
        })}
      </div>
      {!agent.core_management && agent.enrolled && (
        <p className="muted">更新子 agent 后可管理核心</p>
      )}
      <ErrorText error={error} />
    </>
  );
}
