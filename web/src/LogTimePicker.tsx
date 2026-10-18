import { useEffect, useId, useRef, useState } from "react";
import { CalendarDays, ChevronDown, X } from "lucide-react";
import LogCalendar, { monthAt, useCalendarPosition } from "./LogCalendar";
import { rangeStart } from "./traffic";

type Preset = "month" | "7d" | "30d";
export type LogTimeRange = { kind: Preset } | { kind: "day"; day: string };
const DAY = 86_400_000,
  OFFSET = 8 * 3_600_000;
const dayAt = (at: number) => new Date(at + OFFSET).toISOString().slice(0, 10);
const labels: Record<Preset, string> = {
  month: "本月",
  "7d": "近 7 天",
  "30d": "近 30 天",
};
export function timeBounds(value: LogTimeRange, now = Date.now()) {
  if (value.kind === "day") {
    const from = Date.parse(`${value.day}T00:00:00+08:00`);
    return { from, to: from + DAY - 1 };
  }
  return {
    from: rangeStart(
      value.kind === "month" ? "month" : value.kind === "7d" ? 7 : 30,
      now,
    ),
    to: now,
  };
}
export default function LogTimePicker({
  value,
  onChange,
}: {
  value: LogTimeRange;
  onChange: (value: LogTimeRange) => void;
}) {
  const [open, setOpen] = useState(false),
    [month, setMonth] = useState(0);
  const ref = useRef<HTMLDivElement>(null),
    trigger = useRef<HTMLButtonElement>(null),
    panel = useRef<HTMLDivElement>(null),
    id = useId();
  const position = useCalendarPosition(open, month, trigger, panel);
  const selected = value.kind === "day" ? value.day : "";
  const label =
    value.kind === "day" ? value.day.replaceAll("-", "/") : labels[value.kind];
  useEffect(() => {
    if (!open) return;
    function outside(e: PointerEvent) {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    }
    function escape(e: KeyboardEvent) {
      if (e.key === "Escape") {
        setOpen(false);
        trigger.current?.focus();
      }
    }
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  function choose(next: LogTimeRange) {
    onChange(next);
    setOpen(false);
    trigger.current?.focus();
  }
  return (
    <div className="log-time-picker" ref={ref}>
      <button
        ref={trigger}
        type="button"
        className="log-time-trigger"
        aria-label={`时间范围：${label}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => {
          if (!open) setMonth(monthAt(selected || dayAt(Date.now())));
          setOpen((v) => !v);
        }}
      >
        <CalendarDays />
        <span>{label}</span>
        <ChevronDown />
      </button>
      {open && (
        <div
          ref={panel}
          id={id}
          className="log-time-popover"
          role="dialog"
          aria-label="选择日期"
          style={position}
        >
          <div className="log-time-heading">
            <strong>选择日期</strong>
            <button
              type="button"
              className="icon-button"
              aria-label="关闭日期选择"
              onClick={() => {
                setOpen(false);
                trigger.current?.focus();
              }}
            >
              <X />
            </button>
          </div>
          <div className="log-time-presets">
            {(["month", "7d", "30d"] as const).map((kind) => (
              <button
                key={kind}
                type="button"
                aria-pressed={value.kind === kind}
                onClick={() => choose({ kind })}
              >
                {labels[kind]}
              </button>
            ))}
          </div>
          <LogCalendar
            from={selected}
            to={selected}
            today={dayAt(Date.now())}
            month={month}
            onMonth={setMonth}
            onSelect={(day) => choose({ kind: "day", day })}
          />
        </div>
      )}
    </div>
  );
}
