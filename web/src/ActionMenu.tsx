import { ChevronDown, Play, Square } from "lucide-react";
import { PendingButton } from "./components";
import "./ActionMenu.css";
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

export function ActionMenuPower({ enabled, busy, disabled = false, label, onToggle, startBlocked }: {
  enabled: boolean;
  busy: boolean;
  disabled?: boolean;
  label: string;
  onToggle: () => void;
  startBlocked?: string;
}) {
  return (
    <div className="menu-power-actions" role="group" aria-label="运行操作">
      <PendingButton role="menuitem" aria-label={`启动${label}`} title={startBlocked} busy={busy} disabled={disabled || enabled || !!startBlocked} onClick={onToggle}>
        <Play /><span>启动</span>
      </PendingButton>
      <PendingButton role="menuitem" aria-label={`停止${label}`} busy={busy} disabled={disabled || !enabled} onClick={onToggle}>
        <Square /><span>停止</span>
      </PendingButton>
    </div>
  );
}

// Matches Lightsail-Panel's instance context menu and keyboard behavior.
export function ActionMenu({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ left: 0, top: 0, maxHeight: 320 });
  const trigger = useRef<HTMLButtonElement>(null),
    menu = useRef<HTMLDivElement>(null);
  const close = () => {
    setOpen(false);
    trigger.current?.focus({ preventScroll: true });
  };
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = trigger.current?.getBoundingClientRect();
      if (!rect) return;
      const width = menu.current?.getBoundingClientRect().width || 233,
        height = (menu.current?.scrollHeight || 320) + 12,
        below = innerHeight - rect.bottom - 12,
        above = rect.top - 12;
      const upward = below < height && above > below,
        maxHeight = Math.min(height, Math.max(below, above));
      setPosition({
        left: Math.max(
          8,
          Math.min(
            rect.right - width,
            document.documentElement.clientWidth - width - 8,
          ),
        ),
        top: upward ? Math.max(8, rect.top - maxHeight - 5) : rect.bottom + 5,
        maxHeight,
      });
    };
    place();
    window.addEventListener("resize", place);
    document.addEventListener("scroll", place, true);
    menu.current
      ?.querySelector<HTMLButtonElement>("button:not(:disabled)")
      ?.focus({ preventScroll: true });
    return () => {
      window.removeEventListener("resize", place);
      document.removeEventListener("scroll", place, true);
    };
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => {
      if (
        !trigger.current?.contains(e.target as Node) &&
        !menu.current?.contains(e.target as Node)
      )
        setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  return (
    <>
      <button
        ref={trigger}
        type="button"
        className="button small instance-menu-button"
        aria-label={label + " 操作"}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        操作
        <ChevronDown size={13} />
      </button>
      {open &&
        createPortal(
          <div
            ref={menu}
            role="menu"
            aria-label={label + " 操作菜单"}
            className="context-menu instance-menu"
            style={position}
            onClick={(e) => {
              if ((e.target as HTMLElement).closest("button")) close();
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                close();
                return;
              }
              if (e.key === "Tab") {
                close();
                return;
              }
              if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key))
                return;
              e.preventDefault();
              const items = Array.from(
                  menu.current?.querySelectorAll<HTMLButtonElement>(
                    "button:not(:disabled)",
                  ) || [],
                ),
                n = items.indexOf(document.activeElement as HTMLButtonElement);
              const next =
                e.key === "Home"
                  ? 0
                  : e.key === "End"
                    ? items.length - 1
                    : (n + (e.key === "ArrowDown" ? 1 : -1) + items.length) %
                      items.length;
              items[next]?.focus();
            }}
          >
            <div className="menu-title">{label}</div>
            {children}
          </div>,
          document.body,
        )}
    </>
  );
}
