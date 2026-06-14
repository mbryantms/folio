"use client";

import { CheckCircle2, AlertCircle, Clock } from "lucide-react";

import { Input } from "@/components/ui/input";
import { validateCron } from "@/lib/api/cron";
import { statusTone } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

export function CronInput({
  id,
  value,
  onChange,
  placeholder = "0 */6 * * *",
}: {
  /** Optional id for the inner input so a sibling `<Label htmlFor>` can
   *  bind to it (shadcn `<FormControl>` wires this itself, but plain-form
   *  callers like the metadata settings tab need it). */
  id?: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const validation = validateCron(value);
  const empty = value.trim() === "";

  return (
    <div className="space-y-2">
      <Input
        id={id}
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
            ? statusTone("success")
            : !validation.ok
              ? statusTone("error")
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
