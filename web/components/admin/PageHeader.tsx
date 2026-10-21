import { Breadcrumbs, type Crumb } from "@/components/shell/Breadcrumbs";
import { cn } from "@/lib/utils";

export function PageHeader({
  title,
  description,
  actions,
  breadcrumbs,
  className,
  actionsClassName,
  phoneDescription = "visible",
}: {
  title: string;
  description?: string;
  actions?: React.ReactNode;
  /** Ancestor trail shown above the title (A2 wayfinding). */
  breadcrumbs?: Crumb[];
  className?: string;
  /** Extra classes for the actions wrapper — e.g. `max-sm:hidden` when a
   *  page swaps its desktop toolbar for a `PhoneControlsBar` below `sm`. */
  actionsClassName?: string;
  /** Below `sm` the header is tightened (smaller title, two-line clamp on
   *  the description); pages whose description is pure flavour can drop
   *  it entirely on phones so the content starts a row sooner. */
  phoneDescription?: "hidden" | "visible";
}) {
  return (
    <header
      className={cn(
        "border-border mb-6 flex flex-wrap items-end justify-between gap-4 border-b pb-4 max-sm:mb-4 max-sm:pb-3",
        className,
      )}
    >
      <div className="min-w-0">
        {breadcrumbs && breadcrumbs.length > 0 ? (
          <Breadcrumbs items={breadcrumbs} className="mb-1.5" />
        ) : null}
        <h1 className="text-foreground text-2xl font-semibold tracking-tight max-sm:text-xl">
          {title}
        </h1>
        {description ? (
          <p
            className={cn(
              "text-muted-foreground mt-1 text-sm max-sm:line-clamp-2 max-sm:text-xs",
              phoneDescription === "hidden" && "max-sm:hidden",
            )}
          >
            {description}
          </p>
        ) : null}
      </div>
      {actions ? (
        // `flex-wrap` so action-heavy pages (e.g. /log, where the
        // header carries a 6-button range selector + Add widget +
        // Reset) gracefully break onto a second row on mobile rather
        // than overflowing the viewport.
        <div
          className={cn("flex flex-wrap items-center gap-2", actionsClassName)}
        >
          {actions}
        </div>
      ) : null}
    </header>
  );
}
