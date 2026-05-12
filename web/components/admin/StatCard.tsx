import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function StatCard({
  label,
  value,
  hint,
  trend,
  className,
}: {
  label: string;
  value: string | number;
  hint?: string;
  trend?: { delta: string; direction: "up" | "down" | "flat" };
  className?: string;
}) {
  return (
    <Card className={cn("overflow-hidden", className)}>
      <CardContent className="flex flex-col gap-2 p-5">
        <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
          {label}
        </p>
        <p className="text-foreground text-3xl font-semibold tracking-tight">
          {value}
        </p>
        <div className="text-muted-foreground flex items-center justify-between text-xs">
          {hint ? <span>{hint}</span> : <span />}
          {trend ? (
            <span
              className={cn(
                "border-border rounded-full border px-2 py-0.5",
                trend.direction === "up" && "text-emerald-400",
                trend.direction === "down" && "text-red-400",
              )}
            >
              {trend.delta}
            </span>
          ) : null}
        </div>
      </CardContent>
    </Card>
  );
}
