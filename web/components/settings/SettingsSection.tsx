import { cn } from "@/lib/utils";

/**
 * Content section inside a settings page. Pairs a heading + description with
 * a card-bordered body so adjacent forms read as siblings rather than
 * competing for attention. Does not include a save button — every settings
 * surface auto-saves; the description should make that clear.
 */
export function SettingsSection({
  title,
  description,
  children,
  className,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("space-y-3", className)}>
      <div>
        <h2 className="text-foreground text-base font-semibold tracking-tight">
          {title}
        </h2>
        {description ? (
          <p className="text-muted-foreground mt-1 text-sm">{description}</p>
        ) : null}
      </div>
      <div className="border-border bg-card rounded-lg border p-5 shadow-sm">
        {children}
      </div>
    </section>
  );
}
