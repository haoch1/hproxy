import { RefreshCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";

/** Show one full turn without delaying fresh data; keep focus and reject repeat clicks. */
export function RefreshButton({
  loading = false,
  disabled = false,
  onClick,
  text = "刷新",
  className = "",
  title,
}: {
  loading?: boolean;
  disabled?: boolean;
  onClick: () => void | Promise<unknown>;
  text?: string;
  className?: string;
  title?: string;
}) {
  const [turning, setTurning] = useState(false);
  const [pending, setPending] = useState(false);
  const gate = useRef({ turning: false, pending: false, mounted: true });
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    gate.current.mounted = true;
    return () => {
      gate.current.mounted = false;
      clearTimeout(timer.current);
    };
  }, []);
  const blocked = loading || turning || pending || disabled;
  return (
    <button
      type="button"
      title={title}
      aria-label={text ? undefined : title}
      className={`button refresh-button ${className}`.trim()}
      disabled={disabled}
      aria-disabled={blocked}
      aria-busy={loading || turning || pending}
      onClick={async () => {
        if (loading || disabled || gate.current.turning || gate.current.pending)
          return;
        gate.current.turning = true;
        gate.current.pending = true;
        setTurning(true);
        setPending(true);
        timer.current = setTimeout(() => {
          gate.current.turning = false;
          if (gate.current.mounted) setTurning(false);
        }, 1000);
        try {
          await onClick();
        } finally {
          gate.current.pending = false;
          if (gate.current.mounted) setPending(false);
        }
      }}
    >
      <span className={turning ? "refresh-turn" : "refresh-icon"}>
        <RefreshCw size={16} />
      </span>
      {text}
      <span className="sr-only">
        {loading || turning || pending ? "，正在刷新" : ""}
      </span>
    </button>
  );
}

