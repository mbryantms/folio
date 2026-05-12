import { cn } from "@/lib/utils";

export type MetadataItem = {
  label: string;
  value: React.ReactNode | null | undefined;
  /** Span 2 columns on the lg breakpoint (use for long values like file paths). */
  wide?: boolean;
};

/**
 * Definition-list metadata grid. Items with null/undefined/empty `value` are
 * skipped so the grid stays compact. Two-column on sm+, configurable max
 * column count via `columns` prop.
 */
export function MetadataGrid({
  items,
  columns = 2,
  className,
}: {
  items: MetadataItem[];
  columns?: 1 | 2 | 3;
  className?: string;
}) {
  const visible = items.filter((it) => {
    if (it.value === null || it.value === undefined) return false;
    if (typeof it.value === "string" && it.value.trim() === "") return false;
    return true;
  });
  if (visible.length === 0) return null;
  const colsClass = {
    1: "grid-cols-1",
    2: "grid-cols-1 sm:grid-cols-2",
    3: "grid-cols-1 sm:grid-cols-2 lg:grid-cols-3",
  }[columns];
  return (
    <dl className={cn("grid gap-x-6 gap-y-4", colsClass, className)}>
      {visible.map((it) => (
        <div
          key={it.label}
          className={cn(
            "flex flex-col gap-1",
            it.wide && "sm:col-span-2 lg:col-span-3",
          )}
        >
          <dt className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
            {it.label}
          </dt>
          <dd className="text-sm leading-6 break-words">{it.value}</dd>
        </div>
      ))}
    </dl>
  );
}
