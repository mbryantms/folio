import { cn } from "@/lib/utils";

export function PageHeader({
  title,
  description,
  actions,
  className,
}: {
  title: string;
  description?: string;
  actions?: React.ReactNode;
  className?: string;
}) {
  return (
    <header
      className={cn(
        "border-border mb-6 flex flex-wrap items-end justify-between gap-4 border-b pb-4",
        className,
      )}
    >
      <div className="min-w-0">
        <h1 className="text-foreground text-2xl font-semibold tracking-tight">
          {title}
        </h1>
        {description ? (
          <p className="text-muted-foreground mt-1 text-sm">{description}</p>
        ) : null}
      </div>
      {actions ? (
        // `flex-wrap` so action-heavy pages (e.g. /log, where the
        // header carries a 6-button range selector + Add widget +
        // Reset) gracefully break onto a second row on mobile rather
        // than overflowing the viewport.
        <div className="flex flex-wrap items-center gap-2">{actions}</div>
      ) : null}
    </header>
  );
}
