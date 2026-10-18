import { bytes } from "./types";
import "./TrafficUsage.css";

export default function TrafficUsage({
  used,
  quota,
  label,
  showUsed = true,
}: {
  used: number;
  quota: number | null;
  label: string;
  showUsed?: boolean;
}) {
  const limited = quota !== null && quota > 0;
  const ratio = limited ? Math.max(0, (used / quota) * 100) : 0;
  return (
    <div className="traffic-usage tabular">
      <div>
        {showUsed ? bytes(used) : limited ? bytes(quota) : "不限量"}
        {showUsed && limited && <span> / {bytes(quota)}</span>}
      </div>
      {limited ? (
        <div className="usage-meter">
          <div
            className="usage-progress"
            role="progressbar"
            aria-label={`${label} 额度使用情况`}
            aria-valuemin={0}
            aria-valuemax={quota}
            aria-valuenow={Math.min(used, quota)}
            aria-valuetext={`已用 ${bytes(used)}，额度 ${bytes(quota)}，${ratio.toFixed(1)}%`}
          >
            <span
              className={ratio >= 100 ? "exhausted" : ratio >= 90 ? "warning" : undefined}
              style={{ width: `${Math.min(100, ratio)}%` }}
            />
          </div>
          <small>{ratio.toFixed(1)}%</small>
        </div>
      ) : (
        showUsed && <small className="usage-unlimited muted">不限量</small>
      )}
    </div>
  );
}
