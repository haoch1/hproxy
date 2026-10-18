import Select from "./Select";
import { useState } from "react";
import type { FormEvent } from "react";
import { ChevronRight, Info } from "lucide-react";
import { post } from "./api";
import type { Agent, Resource, Spec } from "./types";
import {
  online,
  primaryAddress,
  forwardListen,
  defaultPort,
  applicationIssue,
} from "./types";
import { ErrorText, Field, Modal, PendingButton, SecretInput, Submit } from "./components";
import QuotaCycleFields from "./QuotaCycleFields";
import { formatQuotaInput, parseQuotaBytes, quotaDate, quotaDay } from "./quota";
import { nonCredentialAutofill } from "./formAutofill";

export default function ResourceEditor({
  data,
  agents,
  onClose,
  onSaved,
}: {
  data: { resource: Resource; isNew: boolean; isCopy?: boolean; fixedAgentId?: string; initialError?: string };
  agents: Agent[];
  onClose: () => void;
  onSaved: (r: Resource) => Promise<void>;
}) {
  const [r, setR] = useState<Resource>(() => {
    const resource = data.resource;
    if (data.isCopy) return {
      ...resource,
      address_bindings: { public_host: !!resource.address_bindings?.public_host, listen_ip: !!resource.address_bindings?.listen_ip },
      spec: resource.spec.type === "forward" ? {
        ...resource.spec, remote_host: "", remote_host_v6: null, remote_port: 0,
      } : resource.spec,
    };
    return { ...resource, expires_at: null, quota_cycle: resource.quota_cycle === undefined && data.isNew && agents.find((a) => a.id === resource.agent_id)?.quota_cycles
      ? { starts_at: quotaDate(quotaDay(Date.now())), months: 1, days: 0 }
      : resource.quota_cycle };
  }),
    [error, setError] = useState(data.initialError || ""),
    [busy, setBusy] = useState(false),
    [generating, setGenerating] = useState(false);
  const [quotaText, setQuotaText] = useState(() => formatQuotaInput(data.resource.quota_bytes));
  const [targetMode, setTargetMode] = useState<"server" | "manual">(() => {
    return data.resource.address_bindings?.target_agent_id ? "server" : "manual";
  });
  const forward = r.spec.type === "forward" ? r.spec : null;
  const fixedAgentId = data.fixedAgentId || (!data.isNew ? data.resource.agent_id : undefined);
  const selected = agents.find((a) => a.id === (fixedAgentId || r.agent_id));
  const issue = selected ? applicationIssue(selected) : null;
  const target = forward
    ? agents.find((a) => a.id === r.address_bindings?.target_agent_id)
    : undefined;
  const update = <K extends keyof Resource>(key: K, value: Resource[K]) =>
    setR((old) => ({ ...old, [key]: value }));
  const updateSpec = (values: Partial<Spec>) =>
    setR((old) => ({ ...old, spec: { ...old.spec, ...values } as Spec }));
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy || generating) return;
    setBusy(true);
    setError("");
    try {
      const quota_bytes = quotaText === formatQuotaInput(data.resource.quota_bytes)
        ? data.resource.quota_bytes : parseQuotaBytes(quotaText);
      await onSaved({ ...r, agent_id: fixedAgentId || r.agent_id, quota_bytes });
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function generate(kind: string, reset = false) {
    if (busy || generating) return;
    setGenerating(true);
    setError("");
    try {
      const spec = await post<Spec>(`/generate/${kind}`);
      setR((old) => ({
        ...old,
        listen_port: reset ? old.listen_port : defaultPort(spec.type),
        spec:
          reset && old.spec.type === "vless" && spec.type === "vless"
            ? { ...spec, sni: old.spec.sni }
            : spec,
      }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setGenerating(false);
    }
  }
  function bindTarget(id: string | null, values: Partial<Spec> = {}) {
    setR((old) => ({ ...old, address_bindings: {
      public_host: !!old.address_bindings?.public_host,
      listen_ip: !!old.address_bindings?.listen_ip, target_agent_id: id,
    }, spec: { ...old.spec, ...values } as Spec }));
  }
  function chooseTarget(id: string) {
    const a = agents.find((a) => a.id === id);
    if (!a) return;
    bindTarget(id, {
      remote_host: primaryAddress(a),
      remote_host_v6: a.ipv4 && a.ipv6 ? a.ipv6 : null,
    });
  }
  const serverField = (
    <Field label="服务器">
      {fixedAgentId ? (
        <input {...nonCredentialAutofill} value={selected?.name || "服务器已不存在"} readOnly tabIndex={-1} />
      ) : <Select
        value={r.agent_id}
        onChange={(e) => {
          const a = agents.find((a) => a.id === e.target.value);
          setR((old) => ({
            ...old,
            agent_id: e.target.value,
            public_host: old.address_bindings?.public_host ? primaryAddress(a) || old.public_host : old.public_host,
            listen_ip:
              old.address_bindings?.listen_ip ? forwardListen(a) : old.listen_ip,
          }));
        }}
      >
        {agents
          .filter((a) => !a.deleting && (!a.revoked || a.id === r.agent_id))
          .map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
      </Select>}
    </Field>
  );
  const portField = (
    <Field label="端口">
      <input {...nonCredentialAutofill}
        autoFocus={data.isCopy}
        required
        type="number"
        min={1}
        max={65535}
        value={r.listen_port || ""}
        onChange={(e) => update("listen_port", Number(e.target.value))}
      />
    </Field>
  );
  const quotaField = <Field label="流量额度" full={!!forward}>
    <input {...nonCredentialAutofill} type="text" spellCheck={false}
      value={quotaText} placeholder="不限量，例如 500 MB 或 1 TB"
      title="支持 KB、MB、GB、TB 或 K、M、G、T，按 1024 进制计算；纯数字按 GB，留空不限量"
      onChange={(e) => {
        setQuotaText(e.target.value);
        try {
          parseQuotaBytes(e.target.value);
          e.target.setCustomValidity("");
        } catch (error) {
          e.target.setCustomValidity((error as Error).message);
        }
      }} />
  </Field>;
  return (
    <Modal
      title={`${data.isCopy ? "复制" : data.isNew ? "创建" : "编辑"}${forward ? "转发规则" : "代理节点"}`}
      size="resource-editor"
      onClose={onClose}
      wide
    >
      <form {...nonCredentialAutofill} onSubmit={submit}>
        <div className="editor-fields">
          {selected && !online(selected) && (
            <div className="notice">
              <Info />
              <span>
                服务器离线，连接后自动应用配置
              </span>
            </div>
          )}
          <div className="form-grid">
            <Field label="名称">
              <input {...nonCredentialAutofill}
                autoFocus={!data.isCopy}
                required
                maxLength={100}
                value={r.name}
                placeholder={
                  forward ? "例如：香港转发东京" : "例如：香港 Reality"
                }
                onChange={(e) => update("name", e.target.value)}
              />
            </Field>
            {forward ? (
              <Field label="协议">
                <Select
                  value={forward.network}
                  onChange={(e) =>
                    updateSpec({
                      network: e.target.value as "tcp" | "udp" | "tcp_udp",
                    })
                  }
                >
                  <option value="tcp">TCP</option>
                  <option value="udp">UDP</option>
                  <option value="tcp_udp">TCP + UDP</option>
                </Select>
              </Field>
            ) : (
              serverField
            )}
            {!forward && (
              <Field label="节点协议">
                <Select
                  value={r.spec.type}
                  disabled={!data.isNew}
                  pending={generating || busy}
                  onChange={(e) => void generate(e.target.value)}
                >
                  <option value="vless">Vless Reality</option>
                  <option value="shadowsocks">Shadowsocks 2022</option>
                </Select>
              </Field>
            )}
            {!forward && quotaField}
            {!forward && (
              <Field label="地址" help={<button type="button" className="address-follow"
                aria-pressed={!!r.address_bindings?.public_host}
                title="启用后自动跟随服务器 IP；编辑地址后改为手动"
                onClick={() => setR((old) => ({ ...old,
                  public_host: primaryAddress(selected) || old.public_host,
                  address_bindings: { ...old.address_bindings,
                    listen_ip: !!old.address_bindings?.listen_ip,
                    public_host: !old.address_bindings?.public_host },
                }))}>跟随服务器</button>}>
                <input {...nonCredentialAutofill}
                  required
                  value={r.address_bindings?.public_host ? primaryAddress(selected) || r.public_host : r.public_host}
                  placeholder="IP 或域名"
                  onChange={(e) => setR((old) => ({ ...old, public_host: e.target.value.trim(),
                    address_bindings: { ...old.address_bindings,
                      listen_ip: !!old.address_bindings?.listen_ip, public_host: false },
                  }))}
                />
              </Field>
            )}
            {!forward && portField}
            {forward && (
              <>
                <section className="forward-endpoint full" aria-label="中转">
                  <div className="forward-endpoint-head"><h3>中转</h3></div>
                  <div className="form-grid">
                    {serverField}
                    {portField}
                  </div>
                </section>
                <section className="forward-endpoint full" aria-label="落地">
                  <div className="forward-endpoint-head"><h3>落地</h3>
                  <div
                    className="endpoint-mode segmented"
                    role="group"
                    aria-label="落地地址方式"
                  >
                    <button
                      type="button"
                      aria-pressed={targetMode === "server"}
                      className={targetMode === "server" ? "selected" : ""}
                      onClick={() => {
                        setTargetMode("server");
                        if (!target && !data.isCopy) {
                          const a =
                            agents.find(
                              (a) =>
                                a.id !== r.agent_id &&
                                primaryAddress(a) &&
                                !a.revoked,
                            ) ||
                            agents.find((a) => primaryAddress(a) && !a.revoked);
                          if (a) chooseTarget(a.id);
                        }
                      }}
                    >
                      选择服务器
                    </button>
                    <button
                      type="button"
                      aria-pressed={targetMode === "manual"}
                      className={targetMode === "manual" ? "selected" : ""}
                      onClick={() => {
                        setTargetMode("manual");
                        bindTarget(null, { remote_host_v6: null });
                      }}
                    >
                      指定地址
                    </button>
                  </div>
                  </div>
                  <div className="form-grid">
                    {targetMode === "server" ? (
                      <Field label="服务器">
                        <Select
                          required
                          value={target?.id || ""}
                          onChange={(e) => chooseTarget(e.target.value)}
                        >
                          <option value="" disabled>
                            选择服务器
                          </option>
                          {agents
                            .filter((a) => !a.revoked && !a.deleting)
                            .map((a) => (
                              <option
                                key={a.id}
                                value={a.id}
                                disabled={!primaryAddress(a)}
                              >
                                {a.name}
                              </option>
                            ))}
                        </Select>
                      </Field>
                    ) : (
                      <Field label="地址">
                        <input {...nonCredentialAutofill}
                          required
                          value={forward.remote_host}
                          placeholder="IP 或域名"
                          onChange={(e) =>
                            updateSpec({ remote_host: e.target.value.trim(), remote_host_v6: null })
                          }
                        />
                      </Field>
                    )}
                    <Field label="端口">
                      <input {...nonCredentialAutofill}
                        required
                        type="number"
                        min={1}
                        max={65535}
                        value={forward.remote_port || ""}
                        onChange={(e) =>
                          updateSpec({ remote_port: Number(e.target.value) })
                        }
                      />
                    </Field>
                  </div>
                </section>
              </>
            )}
            {forward && quotaField}
          </div>
          <QuotaCycleFields value={r.quota_cycle} onChange={(value) => update("quota_cycle", value)} disabled={!selected?.quota_cycles} />
          {r.spec.type === "vless" && <Field label="TLS 伪装域名（SNI）">
            <input {...nonCredentialAutofill} required value={r.spec.sni} onChange={(e) => updateSpec({ sni: e.target.value })} />
          </Field>}
          {!forward && (
            <details className="connection-params">
              <summary className="param-summary">
                <span>
                  <ChevronRight />
                  连接参数
                </span>
                <PendingButton
                  type="button"
                  busy={generating || busy}
                  title="保存后生效，旧连接参数会失效"
                  onClick={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    void generate(r.spec.type, true);
                  }}
                >
                  重新生成
                </PendingButton>
              </summary>
              <div className="form-grid">
                {r.spec.type === "vless"
                  ? (
                      ["uuid", "private_key", "public_key", "short_id"] as const
                    ).map((key) => (
                      <Field
                        key={key}
                        label={
                          {
                            uuid: "UUID",
                            private_key: "Reality 私钥",
                            public_key: "Reality 公钥",
                            short_id: "Short ID",
                          }[key]
                        }
                        full
                      >
                        {key === "private_key" || key === "public_key" ? <SecretInput
                          label={{ uuid: "UUID", private_key: "Reality 私钥", public_key: "Reality 公钥", short_id: "Short ID" }[key]}
                          required
                          autoComplete="off"
                          value={r.spec.type === "vless" ? r.spec[key] : ""}
                          onChange={(e) =>
                            updateSpec({ [key]: e.target.value })
                          }
                        /> : <input {...nonCredentialAutofill}
                          required spellCheck={false}
                          value={r.spec.type === "vless" ? r.spec[key] : ""}
                          onChange={(e) => updateSpec({ [key]: e.target.value })}
                        />}
                      </Field>
                    ))
                  : r.spec.type === "shadowsocks" && (
                      <Field label="Shadowsocks 2022 密钥" full>
                        <SecretInput label="Shadowsocks 2022 密钥"
                          required
                          autoComplete="off"
                          value={r.spec.password}
                          onChange={(e) =>
                            updateSpec({ password: e.target.value })
                          }
                        />
                      </Field>
                    )}
              </div>
            </details>
          )}
        </div>
        <ErrorText error={error || (issue?.kind === "failed" ? issue.reason : "") || ""} />
        <div className="form-actions">
          <button type="button" onClick={onClose}>
            取消
          </button>
          <Submit busy={busy || generating} />
        </div>
      </form>
    </Modal>
  );
}
