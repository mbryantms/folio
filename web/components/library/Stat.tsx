import { cn } from "@/lib/utils";

/**
 * Small metric card used in the issue + series page hero stat rows. Falls
 * back to an em-dash when `value` is null/undefined so the row keeps its
 * shape for issues without page counts.
 */
export function Stat({
  label,
  value,
  hint,
  className,
}: {
  label: string;
  value: React.ReactNode | null | undefined;
  hint?: string;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "border-border bg-card flex flex-col gap-1 rounded-md border px-4 py-3",
        className,
      )}
    >
      <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
        {label}
      </span>
      <span className="text-lg leading-tight font-semibold">
        {value ?? "—"}
      </span>
      {hint && <span className="text-muted-foreground text-xs">{hint}</span>}
    </div>
  );
}
