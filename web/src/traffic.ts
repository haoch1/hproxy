import type { Traffic } from "./types";
export const HOUR = 3600000,
  DAY = 24 * HOUR;
export const chinaDay = (at: number) =>
  Math.floor((at + 8 * HOUR) / DAY) * DAY - 8 * HOUR;
export type TrafficRange = "month" | 7 | 30;
export function rangeStart(range: TrafficRange, at = Date.now()) {
  const date = new Date(at + 8 * HOUR);
  return range === "month"
    ? Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1) - 8 * HOUR
    : chinaDay(at) - (range - 1) * DAY;
}
export function buckets(
  rows: Traffic[],
  range: TrafficRange,
  now = Date.now(),
) {
  const start = rangeStart(range, now),
    n = range === "month" ? new Date(now + 8 * HOUR).getUTCDate() : range;
  const grouped = new Map<number, Traffic>();
  for (const row of rows) {
    const at = chinaDay(row.at),
      p = grouped.get(at) || { at, upload: 0, download: 0 };
    p.upload += row.upload;
    p.download += row.download;
    grouped.set(at, p);
  }
  return Array.from(
    { length: n },
    (_, i) =>
      grouped.get(start + i * DAY) || {
        at: start + i * DAY,
        upload: 0,
        download: 0,
      },
  );
}
