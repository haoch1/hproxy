import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import { PendingButton } from "./components";

const sizes = [20, 50, 100];

export default function LogPagination({
  page,
  pages,
  total,
  pageSize,
  busy,
  ready = true,
  onPage,
  onPageSize,
}: {
  page: number;
  pages: number;
  total: number;
  pageSize: number;
  busy: boolean;
  ready?: boolean;
  onPage: (page: number) => void;
  onPageSize: (size: number) => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null),
    trigger = useRef<HTMLButtonElement>(null),
    menu = useRef<HTMLDivElement>(null),
    menuId = useId();
  const footer = useRef<HTMLDivElement>(null),
    anchor = useRef<number | null>(null),
    pending = useRef(false);
  useLayoutEffect(() => {
    if (busy) return;
    pending.current = false;
    if (anchor.current === null || !footer.current) return;
    const delta = footer.current.getBoundingClientRect().top - anchor.current;
    window.scrollBy({ top: delta, behavior: "instant" });
    anchor.current = null;
  }, [busy, page, pageSize, total]);
  function changePage(next: number) {
    if (busy || pending.current || next < 1 || next > pages) return;
    anchor.current = footer.current?.getBoundingClientRect().top ?? null;
    pending.current = true;
    onPage(next);
  }

  useEffect(() => {
    if (!open) return;
    menu.current
      ?.querySelector<HTMLButtonElement>('[aria-checked="true"]')
      ?.focus();
    function outside(event: PointerEvent) {
      if (!ref.current?.contains(event.target as Node)) setOpen(false);
    }
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);

  return (
    <div ref={footer} className="panel-foot logs-pagination">
      <span className="log-total">{ready ? `共 ${total.toLocaleString()} 条记录` : "记录数 —"}</span>
      <nav className="pagination" aria-label="连接日志分页">
        <div className="page-size" ref={ref}>
          <button
            ref={trigger}
            className="page-size-trigger"
            aria-label={`每页条数：${pageSize} 条 / 页`}
            aria-haspopup="menu"
            aria-expanded={open}
            aria-controls={open ? menuId : undefined}
            onClick={() => setOpen((value) => !value)}
            onKeyDown={(event) => {
              if (event.key === "ArrowUp" || event.key === "ArrowDown") {
                event.preventDefault();
                setOpen(true);
              }
            }}
          >
            <span>{pageSize} 条 / 页</span>
            <ChevronDown className={open ? "is-open" : ""} />
          </button>
          {open && (
            <div
              className="page-size-menu"
              role="menu"
              aria-label="每页条数"
              id={menuId}
              ref={menu}
              onKeyDown={(event) => {
                const options = Array.from(
                  menu.current!.querySelectorAll<HTMLButtonElement>("button"),
                );
                const index = options.indexOf(
                  document.activeElement as HTMLButtonElement,
                );
                if (event.key === "Escape" || event.key === "Tab") {
                  setOpen(false);
                  if (event.key === "Escape") {
                    event.preventDefault();
                    trigger.current?.focus();
                  }
                } else if (
                  ["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)
                ) {
                  event.preventDefault();
                  const next =
                    event.key === "Home"
                      ? 0
                      : event.key === "End"
                        ? options.length - 1
                        : (index +
                            (event.key === "ArrowDown" ? 1 : -1) +
                            options.length) %
                          options.length;
                  options[next]?.focus();
                }
              }}
            >
              {sizes.map((size) => (
                <button
                  key={size}
                  role="menuitemradio"
                  aria-checked={pageSize === size}
                  onClick={() => {
                    if (busy || pending.current || size === pageSize) { setOpen(false); return; }
                    anchor.current = footer.current?.getBoundingClientRect().top ?? null;
                    pending.current = true;
                    onPageSize(size);
                    setOpen(false);
                    trigger.current?.focus();
                  }}
                >
                  <span>{size} 条 / 页</span>
                  {size === pageSize && <Check />}
                </button>
              ))}
            </div>
          )}
        </div>
        <span className="page-position" aria-live="polite">
          {ready ? `第 ${page} / ${pages} 页` : "第 — / — 页"}
        </span>
        <PendingButton disabled={page <= 1} busy={busy} aria-disabled={page <= 1 || busy} onClick={() => changePage(page - 1)}>
          上一页
        </PendingButton>
        <PendingButton
          disabled={page >= pages}
          busy={busy}
          aria-disabled={page >= pages || busy}
          onClick={() => changePage(page + 1)}
        >
          下一页
        </PendingButton>
      </nav>
    </div>
  );
}
