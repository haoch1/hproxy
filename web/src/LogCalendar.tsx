import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent, RefObject } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import Select from "./Select";

const DAY = 86_400_000;
const dayAt = (at: number) => new Date(at).toISOString().slice(0, 10);
const atDay = (day: string) => Date.parse(`${day}T00:00:00Z`);
export const monthAt = (day: string) => {
  const date = new Date(atDay(day));
  return Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1);
};
export const dateLabel = (day: string) =>
  `${Number(day.slice(0, 4))}年${Number(day.slice(5, 7))}月${Number(day.slice(8, 10))}日`;

export function useCalendarPosition(
  open: boolean,
  month: number,
  trigger: RefObject<HTMLButtonElement | null>,
  panel: RefObject<HTMLDivElement | null>,
  alignRight = false,
) {
  const [position, setPosition] = useState({ left: 0, top: 0 });
  const reposition = useCallback(() => {
    const rect = trigger.current?.getBoundingClientRect();
    const box = panel.current?.getBoundingClientRect();
    if (!rect || !box) return;
    const below = window.innerHeight - rect.bottom - 24;
    const top = below >= box.height ? rect.bottom + 8 : rect.top - box.height - 8;
    const left = alignRight ? rect.right - box.width : rect.left;
    setPosition((previous) => {
      const next = {
        left: Math.max(16, Math.min(left, window.innerWidth - box.width - 16)),
        top: Math.max(16, Math.min(top, window.innerHeight - box.height - 16)),
      };
      return previous.left === next.left && previous.top === next.top ? previous : next;
    });
  }, [alignRight, trigger, panel]);
  useLayoutEffect(() => {
    if (open) reposition();
  }, [open, month, reposition]);
  useEffect(() => {
    if (!open) return;
    window.addEventListener("resize", reposition);
    window.addEventListener("scroll", reposition, true);
    return () => {
      window.removeEventListener("resize", reposition);
      window.removeEventListener("scroll", reposition, true);
    };
  }, [open, reposition]);
  return position;
}

export default function LogCalendar({
  from,
  to,
  today,
  month,
  onMonth,
  onSelect,
}: {
  from: string;
  to: string;
  today: string;
  month: number;
  onMonth: (at: number) => void;
  onSelect: (day: string) => void;
}) {
  const [focused, setFocused] = useState(from);
  const focusPending = useRef(false);
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const date = new Date(month);
  const year = date.getUTCFullYear(),
    index = date.getUTCMonth();
  const first = month - ((date.getUTCDay() + 6) % 7) * DAY;
  const days = Array.from({ length: 42 }, (_, i) => dayAt(first + i * DAY));
  const tabDay = days.includes(focused) ? focused : dayAt(month);

  useEffect(() => {
    if (focusPending.current) {
      buttons.current.get(focused)?.focus();
      focusPending.current = false;
    }
  }, [focused, month]);

  function key(e: KeyboardEvent<HTMLButtonElement>, day: string) {
    const at = atDay(day),
      value = new Date(at);
    let next: number;
    switch (e.key) {
      case "ArrowLeft":
        next = at - DAY;
        break;
      case "ArrowRight":
        next = at + DAY;
        break;
      case "ArrowUp":
        next = at - 7 * DAY;
        break;
      case "ArrowDown":
        next = at + 7 * DAY;
        break;
      case "Home":
        next = at - ((value.getUTCDay() + 6) % 7) * DAY;
        break;
      case "End":
        next = at + (6 - ((value.getUTCDay() + 6) % 7)) * DAY;
        break;
      case "PageUp":
        next = Date.UTC(value.getUTCFullYear(), value.getUTCMonth() - 1, 1);
        break;
      case "PageDown":
        next = Date.UTC(value.getUTCFullYear(), value.getUTCMonth() + 1, 1);
        break;
      default:
        return;
    }
    e.preventDefault();
    const target = dayAt(next);
    focusPending.current = true;
    setFocused(target);
    onMonth(monthAt(target));
  }

  return (
    <div className="log-calendar">
      <div className="log-calendar-heading">
        <button
          type="button"
          className="icon-button"
          aria-label="上个月"
          onClick={() => onMonth(Date.UTC(year, index - 1, 1))}
        >
          <ChevronLeft />
        </button>
        <div className="log-calendar-month" role="group" aria-label="选择年月">
          <Select aria-label="年份" value={year} menuClassName="calendar-select-menu" menuMaxHeight={204} menuMinWidth={112}
            onChange={(e) => onMonth(Date.UTC(Number(e.target.value), index, 1))}>
            {Array.from({ length: 101 }, (_, i) => year - 50 + i).map((value) =>
              <option key={value} value={value}>{value} 年</option>)}
          </Select>
          <Select aria-label="月份" value={index} menuClassName="calendar-select-menu" menuMaxHeight={204} menuMinWidth={88}
            onChange={(e) => onMonth(Date.UTC(year, Number(e.target.value), 1))}>
            {Array.from({ length: 12 }, (_, i) =>
              <option key={i} value={i}>{i + 1} 月</option>)}
          </Select>
        </div>
        <button
          type="button"
          className="icon-button"
          aria-label="下个月"
          onClick={() => onMonth(Date.UTC(year, index + 1, 1))}
        >
          <ChevronRight />
        </button>
      </div>
      <div className="log-calendar-grid" role="group" aria-label="选择日期">
        {["一", "二", "三", "四", "五", "六", "日"].map((day) => (
          <span className="log-calendar-weekday" key={day}>
            {day}
          </span>
        ))}
        {days.map((day) => (
          <div
            key={day}
            className={`log-calendar-cell${day > from && day < to ? " is-in-range" : ""}${day === from && from !== to ? " is-range-start" : ""}${day === to && from !== to ? " is-range-end" : ""}`}
          >
            <button
              ref={(el) => {
                if (el) buttons.current.set(day, el);
                else buttons.current.delete(day);
              }}
              type="button"
              aria-label={dateLabel(day)}
              aria-pressed={day === from || day === to}
              aria-current={day === today ? "date" : undefined}
              className={`log-calendar-day${day.slice(0, 7) !== dayAt(month).slice(0, 7) ? " is-other-month" : ""}`}
              tabIndex={day === tabDay ? 0 : -1}
              onFocus={() => setFocused(day)}
              onKeyDown={(e) => key(e, day)}
              onClick={() => onSelect(day)}
            >
              {Number(day.slice(8))}
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
