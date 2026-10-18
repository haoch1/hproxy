import type { ReactNode } from "react";

export function Skeleton({ className = "" }: { className?: string }) {
  return <span className={"skeleton " + className} aria-hidden="true" />;
}

/** Keep the loading and content layers in one place; cached refreshes never hide content. */
export function DataRegion({
  pending,
  placeholder,
  children,
  className = "",
  inline = false,
}: {
  pending: boolean;
  placeholder: ReactNode;
  children: ReactNode;
  className?: string;
  inline?: boolean;
}) {
  const Layer = inline ? "span" : "div";
  return (
    <Layer
      className={
        "data-region " + (inline ? "data-region-inline " : "") + className
      }
      aria-busy={pending}
    >
      <Layer className="data-region-placeholder" aria-hidden="true">
        {placeholder}
      </Layer>
      {pending && (
        <span className="sr-only" role="status">
          正在读取数据…
        </span>
      )}
      <Layer
        className="data-region-content"
        aria-hidden={pending || undefined}
        inert={pending || undefined}
      >
        {children}
      </Layer>
    </Layer>
  );
}

