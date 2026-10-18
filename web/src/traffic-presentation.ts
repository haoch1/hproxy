export type TrafficDay = {
  date: string;
  inBytes: number | null;
  outBytes: number | null;
};

const valid = (value: number | null) =>
  value != null && Number.isFinite(value) && value >= 0 ? value : null;

export function trafficBars(rows: TrafficDay[]) {
  const days = rows
    .map((row) => {
      const inbound = valid(row.inBytes),
        outbound = valid(row.outBytes);
      return {
        date: row.date,
        inbound,
        outbound,
        total:
          inbound === null || outbound === null ? null : inbound + outbound,
      };
    })
    .sort((a, b) => a.date.localeCompare(b.date));
  const maximum = Math.max(1, ...days.map((day) => day.total ?? 0));
  return days.map((day) => ({
    ...day,
    percent: day.total === null ? 0 : (day.total / maximum) * 100,
  }));
}

export function trafficTotals(rows: TrafficDay[]) {
  const bars = trafficBars(rows);
  const sum = (field: "inbound" | "outbound" | "total") =>
    !bars.length || bars.some((row) => row[field] === null)
      ? null
      : bars.reduce((total, row) => total + row[field]!, 0);
  return {
    inbound: sum("inbound"),
    outbound: sum("outbound"),
    total: sum("total"),
  };
}

/** Single-direction monthly usage: compare the two range totals, not daily peaks. */
export function singleDirectionTraffic(rows: TrafficDay[]) {
  const { inbound, outbound } = trafficTotals(rows);
  return inbound === null || outbound === null
    ? null
    : Math.max(inbound, outbound);
}

export function chartTooltipPosition(
  anchor: { x: number; y: number },
  size: { width: number; height: number },
  viewport: { width: number; height: number },
) {
  const gap = 12,
    edge = 8;
  const x =
    anchor.x + gap + size.width <= viewport.width - edge
      ? anchor.x + gap
      : anchor.x - gap - size.width;
  const y =
    anchor.y - gap - size.height >= edge
      ? anchor.y - gap - size.height
      : anchor.y + gap;
  return {
    left: Math.max(edge, Math.min(x, viewport.width - size.width - edge)),
    top: Math.max(edge, Math.min(y, viewport.height - size.height - edge)),
  };
}

const rangeFormatter = new Intl.DateTimeFormat("sv-SE", {
  timeZone: "Asia/Shanghai",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});
export function trafficRange(start?: number, end?: number) {
  return start != null &&
    end != null &&
    Number.isFinite(start) &&
    Number.isFinite(end)
    ? `${rangeFormatter.format(start)} → ${rangeFormatter.format(end)}（UTC+08:00）`
    : "—";
}
