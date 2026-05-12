"use client";

import { CheckCircle2, AlertCircle, Clock } from "lucide-react";

import { Input } from "@/components/ui/input";
import { validateCron } from "@/lib/api/cron";
import { cn } from "@/lib/utils";

export function CronInput({
  value,
  onChange,
  placeholder = "0 */6 * * *",
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const validation = validateCron(value);
  const empty = value.trim() === "";

  return (
    <div className="space-y-2">
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="font-mono"
        aria-invalid={!validation.ok}
      />
      <div
        className={cn(
          "flex items-start gap-2 rounded-md border px-3 py-2 text-xs",
          validation.ok && !empty
            ? "border-emerald-900/40 bg-emerald-950/20 text-emerald-200"
            : !validation.ok
              ? "border-destructive/40 bg-destructive/10 text-destructive"
              : "border-border bg-muted/30 text-muted-foreground",
        )}
      >
        {validation.ok ? (
          empty ? (
            <Clock className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          ) : (
            <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          )
        ) : (
          <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
        )}
        <div className="min-w-0 space-y-1">
          <p className="font-medium">
            {validation.ok ? validation.humanized : validation.error}
          </p>
          {validation.ok && validation.nextRuns.length > 0 ? (
            <p className="text-muted-foreground">
              Next:{" "}
              {validation.nextRuns.map((d) => d.toLocaleString()).join(" · ")}
            </p>
          ) : null}
        </div>
      </div>
    </div>
  );
}
