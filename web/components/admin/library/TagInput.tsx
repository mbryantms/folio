"use client";

import { X } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";

export function TagInput({
  value,
  onChange,
  placeholder = "Add pattern…",
  validate,
}: {
  value: string[];
  onChange: (v: string[]) => void;
  placeholder?: string;
  validate?: (raw: string) => string | null;
}) {
  const [draft, setDraft] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);

  const commit = () => {
    const raw = draft.trim();
    if (!raw) return;
    if (value.includes(raw)) {
      setError("Already added");
      return;
    }
    if (validate) {
      const err = validate(raw);
      if (err) {
        setError(err);
        return;
      }
    }
    onChange([...value, raw]);
    setDraft("");
    setError(null);
  };

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1.5">
        {value.length === 0 ? (
          <span className="text-muted-foreground text-xs">No patterns.</span>
        ) : null}
        {value.map((pattern, i) => (
          <Badge
            key={`${pattern}-${i}`}
            variant="secondary"
            className="gap-1.5 font-mono"
          >
            {pattern}
            <button
              type="button"
              aria-label={`Remove ${pattern}`}
              onClick={() => onChange(value.filter((_, j) => j !== i))}
              className="hover:bg-foreground/10 rounded-full p-0.5"
            >
              <X className="h-3 w-3" />
            </button>
          </Badge>
        ))}
      </div>
      <div className="flex gap-2">
        <Input
          value={draft}
          onChange={(e) => {
            setDraft(e.target.value);
            if (error) setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === ",") {
              e.preventDefault();
              commit();
            }
          }}
          onBlur={commit}
          placeholder={placeholder}
          aria-invalid={!!error}
          className="font-mono text-sm"
        />
      </div>
      {error ? (
        <p className="text-destructive text-xs font-medium">{error}</p>
      ) : null}
    </div>
  );
}
