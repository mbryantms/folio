import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { cn } from "@/lib/utils";

/**
 * A titled block inside a Details tab (issue + series pages). The default
 * card variant uses the app's shadcn card chrome; plain sections keep the
 * heading/content rhythm without adding another border.
 */
export function DetailSection({
  title,
  description,
  children,
  variant = "card",
  className,
  contentClassName,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
  variant?: "card" | "plain";
  className?: string;
  contentClassName?: string;
}) {
  if (variant === "card") {
    return (
      <Card className={className}>
        <CardHeader className="p-4 pb-2">
          <CardTitle className="text-sm">{title}</CardTitle>
          {description && (
            <CardDescription className="text-xs">{description}</CardDescription>
          )}
        </CardHeader>
        <CardContent className={cn("p-4 pt-1", contentClassName)}>
          {children}
        </CardContent>
      </Card>
    );
  }

  return (
    <section className={cn("space-y-3", className)}>
      <div className="space-y-1">
        <h3 className="text-foreground text-sm font-semibold">{title}</h3>
        {description && (
          <p className="text-muted-foreground text-xs">{description}</p>
        )}
      </div>
      <div className={contentClassName}>{children}</div>
    </section>
  );
}

export function DetailSummaryGrid({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("grid grid-cols-2 gap-3 lg:grid-cols-4", className)}>
      {children}
    </div>
  );
}

export function DetailSummaryItem({
  label,
  value,
  hint,
  icon,
}: {
  label: string;
  value: React.ReactNode | null | undefined;
  hint?: React.ReactNode | null;
  icon?: React.ReactNode;
}) {
  return (
    <Card className="rounded-md">
      <CardContent className="flex min-h-24 flex-col justify-between gap-3 p-4">
        <div className="flex items-start justify-between gap-3">
          <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
            {label}
          </span>
          {icon && (
            <span className="text-muted-foreground/70 shrink-0">{icon}</span>
          )}
        </div>
        <div className="min-w-0 space-y-1">
          <p className="text-foreground truncate text-lg leading-tight font-semibold">
            {value ?? "—"}
          </p>
          {hint && (
            <p className="text-muted-foreground truncate text-xs">{hint}</p>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
