"use client";

import { Loader2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { useUpdateSeries } from "@/lib/api/mutations";
import type { SeriesView, UpdateSeriesReq } from "@/lib/api/types";

const STATUS_OPTIONS: Array<{ value: string; label: string }> = [
  { value: "continuing", label: "Continuing" },
  { value: "ended", label: "Ended" },
  { value: "cancelled", label: "Cancelled" },
  { value: "hiatus", label: "Hiatus" },
  { value: "limited", label: "Limited" },
];

/**
 * Series Edit drawer — companion to the per-issue Edit drawer. Surfaces the
 * series-wide fields (status, summary) plus the external-database identifiers
 * (ComicVine / Metron). On save we PATCH `/series/{slug}` and
 * `router.refresh()` so the server-rendered page picks up the new values.
 *
 * Genres + tags are no longer editable here: they're aggregated server-side
 * from each issue's ComicInfo. To change a series-level genre, edit the
 * underlying issues.
 *
 * Visibility is gated by the caller (admin only) — the drawer trusts that
 * the trigger is admin-scoped.
 */
export function SeriesEditDrawer({
  series,
  open,
  onOpenChange,
}: {
  series: SeriesView;
  open: boolean;
  onOpenChange: (next: boolean) => void;
}) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="flex w-full flex-col gap-0 p-0 sm:max-w-xl"
      >
        <SheetHeader className="border-border border-b px-6 py-4">
          <SheetTitle>Edit series</SheetTitle>
          <SheetDescription>
            Series-wide fields. Genres and tags are aggregated from issues —
            edit those at the issue level.
          </SheetDescription>
        </SheetHeader>
        <EditForm series={series} onSaved={() => onOpenChange(false)} />
      </SheetContent>
    </Sheet>
  );
}

type FormState = {
  status: string;
  summary: string;
  comicvine_id: string;
  metron_id: string;
};

function initialState(s: SeriesView): FormState {
  return {
    status: s.status?.toLowerCase() ?? "continuing",
    summary: s.summary ?? "",
    comicvine_id: s.comicvine_id != null ? String(s.comicvine_id) : "",
    metron_id: s.metron_id != null ? String(s.metron_id) : "",
  };
}

function EditForm({
  series,
  onSaved,
}: {
  series: SeriesView;
  onSaved: () => void;
}) {
  const router = useRouter();
  // useUpdateSeries takes the slug; the legacy parameter name `seriesId`
  // is just historical (the route is `/series/{slug}`).
  const update = useUpdateSeries(series.slug);
  const [form, setForm] = useState<FormState>(() => initialState(series));

  const set = <K extends keyof FormState>(key: K, value: FormState[K]) =>
    setForm((s) => ({ ...s, [key]: value }));

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const body = buildBody(series, form);
    if (Object.keys(body).length === 0) {
      onSaved();
      return;
    }
    update.mutate(body, {
      onSuccess: () => {
        router.refresh();
        onSaved();
      },
    });
  };

  return (
    <form onSubmit={onSubmit} className="flex min-h-0 flex-1 flex-col">
      <div className="flex-1 overflow-y-auto px-6 py-5">
        <div className="space-y-8">
          <Section
            title="Identity"
            hint={`Read-only. Series name and slug are managed via admin tools.`}
          >
            <Field label="Series" htmlFor="se-name">
              <Input
                id="se-name"
                value={series.name}
                disabled
                aria-readonly="true"
              />
            </Field>
          </Section>

          <Section title="Publication" hint="Status applies to every issue.">
            <Field label="Status" htmlFor="se-status">
              <NativeSelect
                id="se-status"
                value={form.status}
                onChange={(v) => set("status", v)}
                options={STATUS_OPTIONS}
              />
            </Field>
          </Section>

          <Section
            title="Description"
            hint="Falls back to the first issue's summary on read when blank."
          >
            <Field label="Summary" htmlFor="se-summary">
              <Textarea
                id="se-summary"
                rows={5}
                value={form.summary}
                onChange={(e) => set("summary", e.target.value)}
                placeholder="A short series synopsis."
              />
            </Field>
          </Section>

          <Section
            title="External"
            hint="Database IDs at the series level. ComicVine: volume id; Metron: series id."
          >
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <Field label="ComicVine volume ID" htmlFor="se-comicvine">
                <Input
                  id="se-comicvine"
                  type="number"
                  inputMode="numeric"
                  value={form.comicvine_id}
                  onChange={(e) => set("comicvine_id", e.target.value)}
                  placeholder="e.g. 49901"
                />
              </Field>
              <Field label="Metron series ID" htmlFor="se-metron">
                <Input
                  id="se-metron"
                  type="number"
                  inputMode="numeric"
                  value={form.metron_id}
                  onChange={(e) => set("metron_id", e.target.value)}
                  placeholder="e.g. 1234"
                />
              </Field>
            </div>
          </Section>
        </div>
      </div>

      <div className="border-border flex items-center justify-end gap-2 border-t px-6 py-4">
        <Button
          type="button"
          variant="ghost"
          onClick={onSaved}
          disabled={update.isPending}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={update.isPending}>
          {update.isPending && (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          )}
          Save
        </Button>
      </div>
    </form>
  );
}

function Section({
  title,
  hint,
  children,
}: {
  title: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-3">
      <header>
        <h3 className="text-foreground text-sm font-semibold">{title}</h3>
        {hint && <p className="text-muted-foreground text-xs">{hint}</p>}
      </header>
      <div className="space-y-3">{children}</div>
    </section>
  );
}

function Field({
  label,
  htmlFor,
  children,
}: {
  label: string;
  htmlFor: string;
  children: React.ReactNode;
}) {
  return (
    <div className="grid gap-1.5">
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
    </div>
  );
}

function NativeSelect({
  id,
  value,
  onChange,
  options,
}: {
  id: string;
  value: string;
  onChange: (next: string) => void;
  options: Array<{ value: string; label: string }>;
}) {
  return (
    <select
      id={id}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="border-input bg-background focus-visible:ring-ring flex h-9 w-full rounded-md border px-3 py-1 text-sm shadow-sm transition-colors focus-visible:ring-1 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50"
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

/**
 * Build the PATCH body containing only fields the user actually changed.
 * Empty summary strings collapse to `null` to clear the server value, which
 * makes the API fall back to the first issue's summary on read.
 */
function buildBody(prev: SeriesView, form: FormState): UpdateSeriesReq {
  const body: UpdateSeriesReq = {};

  const nextStatus = form.status.trim().toLowerCase();
  if (nextStatus !== "" && nextStatus !== prev.status?.toLowerCase()) {
    body.status = nextStatus;
  }

  // Summary: prefer current series-level value (which the API may have
  // synthesized from the first issue when the row was null) for diffing.
  // The user typically edits to a meaningful value; the rare "I want to
  // clear" path sends "" → null and falls back to the first-issue value.
  if (didChangeStr(prev.summary, form.summary)) {
    body.summary = emptyToNull(form.summary);
  }

  if (didChangeInt(prev.comicvine_id, form.comicvine_id)) {
    body.comicvine_id = parseIntOrNull(form.comicvine_id);
  }
  if (didChangeInt(prev.metron_id, form.metron_id)) {
    body.metron_id = parseIntOrNull(form.metron_id);
  }

  return body;
}

function didChangeStr(prev: string | null, next: string): boolean {
  return (prev ?? "") !== next.trim();
}

function didChangeInt(prev: number | null, next: string): boolean {
  const trimmed = next.trim();
  if (trimmed === "") return prev != null;
  const n = Number.parseInt(trimmed, 10);
  if (!Number.isFinite(n)) return false;
  return n !== prev;
}

function emptyToNull(value: string): string | null {
  const t = value.trim();
  return t === "" ? null : t;
}

function parseIntOrNull(value: string): number | null {
  const t = value.trim();
  if (t === "") return null;
  const n = Number.parseInt(t, 10);
  return Number.isFinite(n) ? n : null;
}
