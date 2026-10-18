import { useEffect, useId, useRef, useState } from "react";
import { CalendarDays, ChevronDown, X } from "lucide-react";
import LogCalendar, { monthAt, useCalendarPosition } from "./LogCalendar";

const dayAt = (at: number) =>
  new Date(at + 8 * 3_600_000).toISOString().slice(0, 10);

export default function ExpiryDatePicker({
  value,
  onChange,
  id,
  "aria-describedby": describedBy,
  label = "到期日期",
  compact = false,
  disabled = false,
  endOfDay = true,
  clearable = true,
}: {
  value: number | null;
  onChange: (value: number | null) => void;
  id?: string;
  "aria-describedby"?: string;
  label?: string;
  compact?: boolean;
  disabled?: boolean;
  endOfDay?: boolean;
  clearable?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [month, setMonth] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const position = useCalendarPosition(open, month, trigger, panel, compact);
  const popup = useId();
  const day = value ? dayAt(value) : "";
  function close() {
    setOpen(false);
    trigger.current?.focus();
  }
  function choose(next: string) {
    onChange(next ? Date.parse(`${next}T${endOfDay ? "23:59:59.999" : "00:00:00"}+08:00`) : null);
    close();
  }
  useEffect(() => {
    if (!open) return;
    function outside(e: PointerEvent) {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("pointerdown", outside);
    return () => {
      document.removeEventListener("pointerdown", outside);
    };
  }, [open]);
  return (
    <div
      ref={root}
      className={`expiry-picker${compact ? " is-compact" : ""}`}
      onKeyDown={(e) => {
        if (open && e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          close();
        }
      }}
    >
      <button
        ref={trigger}
        id={id}
        type="button"
        disabled={disabled}
        aria-label={compact ? label : undefined}
        title={compact ? `${label}：${day.replaceAll("-", "/")}` : undefined}
        className={`expiry-trigger${day ? " has-date" : ""}`}
        aria-describedby={describedBy}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popup : undefined}
        onClick={() => {
          setMonth(monthAt(day || dayAt(Date.now())));
          setOpen((v) => !v);
        }}
      >
        <CalendarDays />
        {!compact && <><span>{day ? day.replaceAll("-", "/") : "不限期"}</span><ChevronDown /></>}
      </button>
      {open && (
        <div
          ref={panel}
          id={popup}
          className="expiry-popover"
          role="dialog"
          aria-label={`选择${label}`}
          style={position}
        >
          <div className="log-time-heading">
            <strong>{label}</strong>
            <button
              type="button"
              className="icon-button"
              aria-label="关闭日期选择"
              onClick={close}
            >
              <X />
            </button>
          </div>
          <LogCalendar
            from={day}
            to={day}
            today={dayAt(Date.now())}
            month={month}
            onMonth={setMonth}
            onSelect={choose}
          />
          <div className="expiry-footer">
            {clearable && <button
              type="button"
              className="text-button"
              onClick={() => choose("")}
            >
              不限期
            </button>}
            <button
              type="button"
              className="text-button"
              onClick={() => choose(dayAt(Date.now()))}
            >
              今天
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
