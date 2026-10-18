import { useId, useRef, useState } from "react";
import { Info } from "lucide-react";
import type { QuotaCycle } from "./types";
import { Field } from "./components";
import Select from "./Select";
import ExpiryDatePicker from "./ExpiryDatePicker";
import { quotaDate, quotaDay, quotaWindow, validCycle } from "./quota";

const presets = [
  { months: 1, days: 30, label: "月" }, { months: 3, days: 92, label: "季" },
  { months: 6, days: 183, label: "半年" }, { months: 12, days: 365, label: "年" },
];
type Props = { value?: QuotaCycle | null; onChange: (value: QuotaCycle | null) => void; disabled?: boolean };
function CycleControl({ value, onChange, disabled, id, "aria-describedby": describedBy, "aria-labelledby": labelledBy }: Props & {
  id?: string; "aria-describedby"?: string; "aria-labelledby"?: string;
}) {
  const [text, setText] = useState(() => !value ? "-1" : String(value.days || presets.find((p) => p.months === value.months)?.days || Math.round(value.months * 365 / 12)));
  const starts_at = value?.starts_at ?? quotaDate(quotaDay(Date.now()));
  return <div className="quota-cycle-controls">
    <Select id={id} aria-describedby={describedBy} aria-labelledby={labelledBy}
      menuMaxWidth={260}
      value={!value ? "once" : value.months ? String(value.months) : "custom"} disabled={disabled}
      title={disabled ? "更新子 agent 后可设置流量周期" : "下拉选择周期，直接输入自定义天数，-1 表示一次性"}
      editable={{ text, placeholder: "一次性（-1）", validate: (next) => !next || next === "-1" || (/^\d+$/.test(next) && Number(next) >= 1 && Number(next) <= 36500) ? "" : "请选择周期，或输入 1–36500 的整数天数，-1 表示一次性",
        onTextChange: (next) => { setText(next); onChange(!next || next === "-1" ? null : { starts_at, months: 0, days: Number(next) || 0 }); } }}
      onChange={(e) => {
        if (e.target.value === "once") { setText("-1"); onChange(null); return; }
        const preset = presets.find((p) => String(p.months) === e.target.value);
        if (preset) { setText(String(preset.days)); onChange({ starts_at, months: preset.months, days: 0 }); }
      }}>
      {presets.map((p) => <option key={p.months} value={p.months}>{p.label}</option>)}
      <option value="once">一次性</option>
    </Select>
    <ExpiryDatePicker compact disabled={disabled || !value || !validCycle(value)} label="周期开始日期" endOfDay={false} clearable={false}
      value={value?.starts_at ?? starts_at} onChange={(next) => { if (value && next !== null) onChange({ ...value, starts_at: next }); }} />
  </div>;
}
function CycleHelp() {
  const id = useId();
  const popup = useRef<HTMLDivElement>(null);
  return <>
    <button type="button" className="field-help-button" aria-label="流量周期说明" popoverTarget={id}
      onClick={(e) => {
        e.preventDefault();
        const panel = popup.current;
        if (!panel) return;
        if (panel.matches(":popover-open")) { panel.hidePopover(); return; }
        panel.showPopover();
        const trigger = e.currentTarget.getBoundingClientRect(), box = panel.getBoundingClientRect();
        const below = window.innerHeight - trigger.bottom - 24;
        const top = below >= box.height ? trigger.bottom + 8 : trigger.top - box.height - 8;
        panel.style.left = `${Math.max(16, Math.min(trigger.left, window.innerWidth - box.width - 16))}px`;
        panel.style.top = `${Math.max(16, Math.min(top, window.innerHeight - box.height - 16))}px`;
      }}><Info /></button>
    <div ref={popup} id={id} popover="auto" className="field-help-popover">
      <h3>流量周期规则</h3>
      <section>
        <h4>按预设周期（日历重置）</h4>
        <p>下拉选择月、季、半年、年，按对应的日历周期重置流量额度</p>
        <ul><li>常用数值：30（月）、92（季）、183（半年）、365（年）</li>
          <li>示例：选择月，周期从 3 月 15 日开始，下次重置为 4 月 15 日</li></ul>
      </section>
      <section>
        <h4>按自定义天数</h4>
        <p>直接输入天数（如 7、45、100），将严格按天数重置</p>
        <ul><li>示例：输入 45，周期从 3 月 15 日开始，下次重置为 4 月 29 日（45 天后）</li></ul>
      </section>
      <section>
        <h4>一次性额度</h4>
        <p>输入 -1 表示一次性额度，不再定期重置，额度用完后停用</p>
      </section>
    </div>
  </>;
}
export default function QuotaCycleFields(props: Props) {
  const { value } = props;
  const hint = !value ? "一次性额度，不自动重置" : validCycle(value)
    ? `下次重置 ${quotaDay(quotaWindow(value)[1]).replaceAll("-", "/")}` : "\u00a0";
  return <div className="form-grid">
    <Field full label="流量周期" help={<CycleHelp />} hint={hint}><CycleControl {...props} /></Field>
  </div>;
}
