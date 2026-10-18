import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { createPortal } from "react-dom";
import {
  chartTooltipPosition,
  trafficBars,
  type TrafficDay,
} from "./traffic-presentation";
import { bytes as formatBytes } from "./types";
const bytes = (value?: number | null) => value == null ? "—" : formatBytes(value);
import { DataRegion, Skeleton } from "./DataRegion";

export default function DailyTrafficChart({
  rows,
  loading,
}: {
  rows: TrafficDay[];
  loading: boolean;
}) {
  const bars = trafficBars(rows);
  const [selected, setSelected] = useState<string | null>(null);
  const tooltipId = useId();
  const chart = useRef<HTMLElement>(null);
  const tooltip = useRef<HTMLDivElement>(null);
  const anchor = useRef({ x: 0, y: 0 });
  const frame = useRef(0);
  const active = bars.find((day) => day.date === selected);

  function place() {
    if (!tooltip.current) return;
    const position = chartTooltipPosition(
      anchor.current,
      {
        width: tooltip.current.offsetWidth,
        height: tooltip.current.offsetHeight,
      },
      { width: window.innerWidth, height: window.innerHeight },
    );
    tooltip.current.style.left = position.left + "px";
    tooltip.current.style.top = position.top + "px";
  }
  function select(date: string, x: number, y: number) {
    anchor.current = { x, y };
    setSelected(date);
    if (!frame.current)
      frame.current = requestAnimationFrame(() => {
        frame.current = 0;
        place();
      });
  }
  useLayoutEffect(place, [selected]);
  useEffect(() => {
    const hide = () => setSelected(null);
    const followFocus = () => {
      const focused = chart.current?.querySelector<HTMLButtonElement>(
        ".daily-bar:focus-visible",
      );
      if (!focused?.dataset.date) return hide();
      const rect = focused.getBoundingClientRect();
      if (rect.bottom <= 0 || rect.top >= window.innerHeight) return hide();
      select(
        focused.dataset.date,
        rect.left + rect.width / 2,
        rect.top + rect.height / 2,
      );
    };
    window.addEventListener("scroll", followFocus, {
      capture: true,
      passive: true,
    });
    window.addEventListener("resize", hide);
    return () => {
      cancelAnimationFrame(frame.current);
      window.removeEventListener("scroll", followFocus, true);
      window.removeEventListener("resize", hide);
    };
  }, []);

  return (
    <section
      ref={chart}
      className="daily-chart"
      aria-label="每日流量图表"
      onPointerLeave={(event) => {
        if (!event.currentTarget.querySelector(":focus-visible"))
          setSelected(null);
      }}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget))
          setSelected(null);
      }}
    >
      <div className="daily-chart-heading">
        <h2>每日流量</h2>
        <span>入站 + 出站</span>
      </div>
      <DataRegion
        pending={loading}
        placeholder={
          <div className="chart-placeholder">
            {Array.from({ length: 15 }, (_, i) => (
              <Skeleton key={i} />
            ))}
          </div>
        }
      >
        {bars.length ? (
          <div className="daily-chart-scroll">
            <div
              className="daily-bars"
              style={{ "--days": bars.length } as CSSProperties}
            >
              {bars.map((day, index) => (
                <button
                  key={day.date}
                  className="daily-bar"
                  data-date={day.date}
                  data-active={active?.date === day.date || undefined}
                  data-missing={day.total === null || undefined}
                  tabIndex={
                    day.date === (active?.date ?? bars[bars.length - 1].date)
                      ? 0
                      : -1
                  }
                  aria-label={`${day.date}，入站 ${bytes(day.inbound)}，出站 ${bytes(day.outbound)}，合计 ${bytes(day.total)}`}
                  aria-describedby={
                    active?.date === day.date ? tooltipId : undefined
                  }
                  onPointerMove={(event) => {
                    if (event.pointerType !== "touch")
                      select(day.date, event.clientX, event.clientY);
                  }}
                  onFocus={(event) => {
                    const rect = event.currentTarget.getBoundingClientRect();
                    select(
                      day.date,
                      rect.left + rect.width / 2,
                      rect.top + rect.height / 2,
                    );
                  }}
                  onClick={(event) => {
                    const rect = event.currentTarget.getBoundingClientRect();
                    select(
                      day.date,
                      event.detail ? event.clientX : rect.left + rect.width / 2,
                      event.detail ? event.clientY : rect.top + rect.height / 2,
                    );
                  }}
                  onKeyDown={(event) => {
                    const next =
                      event.key === "ArrowLeft"
                        ? index - 1
                        : event.key === "ArrowRight"
                          ? index + 1
                          : event.key === "Home"
                            ? 0
                            : event.key === "End"
                              ? bars.length - 1
                              : null;
                    if (event.key === "Escape") setSelected(null);
                    if (next === null) return;
                    event.preventDefault();
                    event.currentTarget.parentElement
                      ?.querySelectorAll<HTMLButtonElement>("button")
                      [Math.max(0, Math.min(bars.length - 1, next))]?.focus();
                  }}
                >
                  <span className="daily-bar-track" aria-hidden="true">
                    <span
                      className="daily-bar-fill"
                      style={{ height: `${day.percent}%` }}
                    />
                    {day.total === null && (
                      <span className="daily-bar-missing">—</span>
                    )}
                  </span>
                  <span className="daily-bar-date" aria-hidden="true">
                    {day.date.slice(5)}
                  </span>
                </button>
              ))}
            </div>
          </div>
        ) : (
          <div className="daily-chart-empty" role="status">
            暂无流量数据
          </div>
        )}
      </DataRegion>
      {createPortal(
        <div
          ref={tooltip}
          id={tooltipId}
          role="tooltip"
          className="daily-chart-tooltip"
          style={{ visibility: active ? "visible" : "hidden" }}
          aria-hidden={!active}
        >
          <strong>{active?.date ?? "\u00a0"}</strong>
          <dl>
            <div>
              <dt>入站</dt>
              <dd>{bytes(active?.inbound)}</dd>
            </div>
            <div>
              <dt>出站</dt>
              <dd>{bytes(active?.outbound)}</dd>
            </div>
            <div>
              <dt>合计</dt>
              <dd>{bytes(active?.total)}</dd>
            </div>
          </dl>
        </div>,
        document.body,
      )}
    </section>
  );
}
