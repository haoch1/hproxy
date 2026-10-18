import type { Counter, QuotaBaseline, QuotaCycle, Resource } from "./types";

const DAY = 86_400_000, ZONE = 8 * 3_600_000;
export function parseQuotaBytes(text: string): number | null {
  if (!text.trim()) return null;
  const match = /^(\d+(?:\.\d*)?|\.\d+)\s*([KMGT]B?|B)?$/i.exec(text.trim());
  if (!match) throw new Error("流量额度格式无效，请输入如 500 MB、1 GB 或 2 TB");
  const units: Record<string, number> = { B: 1, K: 1024, M: 1024 ** 2, G: 1024 ** 3, T: 1024 ** 4 };
  const bytes = Math.round(Number(match[1]) * units[(match[2] || "G")[0].toUpperCase()]);
  if (!Number.isSafeInteger(bytes)) throw new Error("流量额度过大，请调小数值");
  if (bytes <= 0) throw new Error("流量额度必须大于零，留空表示不限量");
  return bytes;
}
export function formatQuotaInput(bytes: number | null): string {
  if (bytes === null) return "";
  const n = Math.max(0, Math.min(4, Math.floor(Math.log2(bytes) / 10)));
  return `${Number((bytes / 1024 ** n).toFixed(2))}${["B", "K", "M", "G", "T"][n]}`;
}

export const quotaDay = (at: number) => new Date(at + ZONE).toISOString().slice(0, 10);
export const quotaDate = (day: string) => Date.parse(`${day}T00:00:00+08:00`);
export const sameCycle = (a: QuotaCycle, b: QuotaCycle) => a.starts_at === b.starts_at && a.months === b.months && a.days === b.days;
export function validCycle(cycle: QuotaCycle): boolean {
  return Number.isSafeInteger(cycle.starts_at) && cycle.starts_at >= 946_656_000_000 && cycle.starts_at < 4_133_952_000_000 &&
    Number.isInteger(cycle.months) && Number.isInteger(cycle.days) &&
    ((cycle.months > 0 && cycle.months <= 60 && cycle.days === 0) ||
      (cycle.days > 0 && cycle.days <= 36_500 && cycle.months === 0));
}
export function quotaWindow(cycle: QuotaCycle, at = Date.now()): [number, number] {
  if (at < cycle.starts_at) return [0, cycle.starts_at];
  if (cycle.days) {
    const width = cycle.days * DAY, start = cycle.starts_at + Math.floor((at - cycle.starts_at) / width) * width;
    return [start, start + width];
  }
  const anchor = new Date(cycle.starts_at + ZONE), current = new Date(at + ZONE);
  let step = Math.floor(((current.getUTCFullYear() - anchor.getUTCFullYear()) * 12 + current.getUTCMonth() - anchor.getUTCMonth()) / cycle.months);
  function boundary(n: number) {
    const date = new Date(anchor);
    date.setUTCDate(1);
    date.setUTCMonth(anchor.getUTCMonth() + n * cycle.months);
    const last = new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 0)).getUTCDate();
    date.setUTCDate(Math.min(anchor.getUTCDate(), last));
    return date.getTime() - ZONE;
  }
  if (boundary(step) > at) step--;
  return [boundary(step), boundary(step + 1)];
}
export function quotaCounter(resource: Resource, total: Counter, baseline?: QuotaBaseline): Counter {
  const cycle = resource.quota_cycle;
  if (!cycle || !baseline) return total;
  if (sameCycle(baseline.cycle, cycle) && quotaWindow(cycle)[0] > baseline.start)
    return { upload: 0, download: 0 };
  return { upload: Math.max(0, total.upload - baseline.counter.upload), download: Math.max(0, total.download - baseline.counter.download) };
}
export function cycleLabel(cycle: QuotaCycle): string {
  const labels: Record<number, string> = { 1: "每月", 3: "每季", 6: "每半年", 12: "每年", 24: "每两年", 36: "每三年", 60: "每五年" };
  return cycle.days ? `每 ${cycle.days} 天` : (labels[cycle.months] || `每 ${cycle.months} 个月`);
}
