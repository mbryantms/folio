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

/** Reading-direction override at the series level. The empty-string
 *  value represents "Auto" (= NULL on the row, defer to user pref /
 *  library default at read time). `manga-and-bulk-metadata-1.0` M2.
 */
const READING_DIRECTION_OPTIONS: Array<{ value: string; label: string }> = [
  { value: "", label: "Auto (inherit)" },
  { value: "ltr", label: "Left-to-right" },
  { value: "rtl", label: "Right-to-left (manga)" },
  { value: "ttb", label: "Vertical (webtoon)" },
];

/** OCR language override at the series level. Empty string = "Auto"
 *  (= NULL on the row; the OCR handler infers manga from an `rtl`
 *  reading direction at request time). OCR rework 1.0. */
const TEXT_LANGUAGE_OPTIONS: Array<{ value: string; label: string }> = [
  { value: "", label: "Auto (infer from direction)" },
  { value: "western", label: "Western (Latin script)" },
  { value: "manga", label: "Japanese (manga)" },
];

/**
 * Series Edit drawer — companion to the per-issue Edit drawer. Surfaces the
 * series-wide fields (status, summary, reading direction). Provider IDs
 * (ComicVine volume, Metron series, etc.) live exclusively in the "External
 * IDs" tab on the series page — the External IDs tab is the single typed-ID
 * editor for both surfaces. On save we PATCH `/series/{slug}` and
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
  reading_direction: string;
  text_language: string;
  summary: string;
};

function initialState(s: SeriesView): FormState {
  return {
    status: s.status?.toLowerCase() ?? "continuing",
    reading_direction: s.reading_direction ?? "",
    text_language: s.text_language ?? "",
    summary: s.summary ?? "",
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
            title="Reading"
            hint="ComicInfo Manga=YesAndRightToLeft on an issue still wins. Auto = defer to your account default and the library."
          >
            <Field label="Reading direction" htmlFor="se-direction">
              <NativeSelect
                id="se-direction"
                value={form.reading_direction}
                onChange={(v) => set("reading_direction", v)}
                options={READING_DIRECTION_OPTIONS}
              />
            </Field>
            <Field label="Text language (OCR)" htmlFor="se-text-language">
              <NativeSelect
                id="se-text-language"
                value={form.text_language}
                onChange={(v) => set("text_language", v)}
                options={TEXT_LANGUAGE_OPTIONS}
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

          {/* External-database IDs (ComicVine volume, Metron series,
              GCD, etc.) live exclusively in the series page's
              "External IDs" tab so the typed-ID surface is a single
              editor instead of two competing ones. */}
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

  // Reading direction: "" = Auto (clear server override). Round-trip
  // through the same emptyToNull treatment so an explicit clear is a
  // PATCH with `reading_direction: null`.
  const prevDir = prev.reading_direction ?? "";
  if (form.reading_direction !== prevDir) {
    body.reading_direction = emptyToNull(form.reading_direction);
  }

  // OCR text language: same empty-string-means-Auto round-trip.
  const prevLang = prev.text_language ?? "";
  if (form.text_language !== prevLang) {
    body.text_language = emptyToNull(form.text_language);
  }

  // Summary: prefer current series-level value (which the API may have
  // synthesized from the first issue when the row was null) for diffing.
  // The user typically edits to a meaningful value; the rare "I want to
  // clear" path sends "" → null and falls back to the first-issue value.
  if (didChangeStr(prev.summary, form.summary)) {
    body.summary = emptyToNull(form.summary);
  }

  return body;
}

function didChangeStr(prev: string | null | undefined, next: string): boolean {
  return (prev ?? "") !== next.trim();
}

function emptyToNull(value: string): string | null {
  const t = value.trim();
  return t === "" ? null : t;
}
