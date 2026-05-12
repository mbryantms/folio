"use client";

import { Loader2, Plus, Trash2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
import {
  useForceRecreateIssuePageMap,
  useUpdateIssue,
  useUpdateSeries,
} from "@/lib/api/mutations";
import type {
  IssueDetailView,
  IssueLink,
  SeriesView,
  UpdateIssueReq,
  UpdateSeriesReq,
} from "@/lib/api/types";
import type { ReadState } from "@/lib/reading-state";

import { IssueSettingsMenu } from "./IssueSettingsMenu";

type FormLink = { label: string; url: string };

const SERIES_STATUS_OPTIONS: Array<{ value: string; label: string }> = [
  { value: "", label: "—" },
  { value: "continuing", label: "Continuing" },
  { value: "ended", label: "Ended" },
  { value: "cancelled", label: "Cancelled" },
  { value: "hiatus", label: "Hiatus" },
  { value: "limited", label: "Limited" },
];

/**
 * Issue page action bar. Renders the consolidated `IssueSettingsMenu` for
 * everyone (it has user-facing items like "Mark as read") and owns the
 * edit-drawer state so the menu's "Edit issue" item can pop the sheet
 * (the menu closes itself on select, so the sheet can't live as a
 * `<SheetTrigger>` inside it). Read-state flows in from the RSC so the
 * menu's "Read from beginning" item can switch label / behavior without
 * a client-side progress fetch.
 */
export function IssueActions({
  issue,
  series,
  readState,
}: {
  issue: IssueDetailView;
  /** Parent series — null when the breadcrumb fetch failed. The drawer's
   *  series-status field is hidden when the series isn't available. */
  series: SeriesView | null;
  readState: ReadState;
}) {
  const [editOpen, setEditOpen] = useState(false);
  const [confirmForceRecreate, setConfirmForceRecreate] = useState(false);
  const forceRecreatePageMap = useForceRecreateIssuePageMap(
    issue.series_slug,
    issue.slug,
    issue.library_id,
  );

  return (
    <>
      <IssueSettingsMenu
        issue={issue}
        readState={readState}
        onEdit={() => setEditOpen(true)}
        onForceRecreatePageMap={() => setConfirmForceRecreate(true)}
      />
      <EditSheet
        issue={issue}
        series={series}
        open={editOpen}
        onOpenChange={setEditOpen}
      />
      <AlertDialog
        open={confirmForceRecreate}
        onOpenChange={setConfirmForceRecreate}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rebuild all page thumbnails?</AlertDialogTitle>
            <AlertDialogDescription>
              Every per-page strip thumbnail for this issue is deleted from disk
              and re-encoded from the source archive. The cover thumbnail is
              preserved. Use this when the existing strips are stale or
              corrupted; otherwise prefer &ldquo;Fill missing page
              thumbnails&rdquo;, which only encodes pages that aren&apos;t
              already on disk.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => forceRecreatePageMap.mutate()}
              disabled={forceRecreatePageMap.isPending}
            >
              Rebuild all
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function EditSheet({
  issue,
  series,
  open,
  onOpenChange,
}: {
  issue: IssueDetailView;
  series: SeriesView | null;
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
          <SheetTitle>Edit issue</SheetTitle>
          <SheetDescription>
            Override fields in the database — the source file is never modified.
            Edits are sticky and a future metadata refresh will not overwrite
            them.
          </SheetDescription>
        </SheetHeader>
        <EditForm
          issue={issue}
          series={series}
          onSaved={() => onOpenChange(false)}
        />
      </SheetContent>
    </Sheet>
  );
}

type FormState = {
  title: string;
  number: string;
  volume: string;
  year: string;
  month: string;
  day: string;
  summary: string;
  notes: string;
  publisher: string;
  imprint: string;
  // Credits
  writer: string;
  penciller: string;
  inker: string;
  colorist: string;
  letterer: string;
  cover_artist: string;
  editor: string;
  translator: string;
  // Cast / setting / story
  characters: string;
  teams: string;
  locations: string;
  alternate_series: string;
  story_arc: string;
  story_arc_number: string;
  // Classification
  genre: string;
  tags: string;
  language_code: string;
  age_rating: string;
  format: string;
  black_and_white: "" | "yes" | "no";
  manga: "" | "Yes" | "YesAndRightToLeft" | "No";
  // Ordering / external
  sort_number: string;
  web_url: string;
  gtin: string;
  comicvine_id: string;
  metron_id: string;
  // Series-level convenience: edits flow to PATCH /series/{slug} on save.
  series_status: string;
};

function initialState(
  issue: IssueDetailView,
  seriesStatus: string | null,
): FormState {
  return {
    title: issue.title ?? "",
    number: issue.number ?? "",
    volume: issue.volume != null ? String(issue.volume) : "",
    year: issue.year != null ? String(issue.year) : "",
    month: issue.month != null ? String(issue.month) : "",
    day: issue.day != null ? String(issue.day) : "",
    summary: issue.summary ?? "",
    notes: issue.notes ?? "",
    publisher: issue.publisher ?? "",
    imprint: issue.imprint ?? "",
    writer: issue.writer ?? "",
    penciller: issue.penciller ?? "",
    inker: issue.inker ?? "",
    colorist: issue.colorist ?? "",
    letterer: issue.letterer ?? "",
    cover_artist: issue.cover_artist ?? "",
    editor: issue.editor ?? "",
    translator: issue.translator ?? "",
    characters: issue.characters ?? "",
    teams: issue.teams ?? "",
    locations: issue.locations ?? "",
    alternate_series: issue.alternate_series ?? "",
    story_arc: issue.story_arc ?? "",
    story_arc_number: issue.story_arc_number ?? "",
    genre: issue.genre ?? "",
    tags: issue.tags ?? "",
    language_code: issue.language_code ?? "",
    age_rating: issue.age_rating ?? "",
    format: issue.format ?? "",
    black_and_white:
      issue.black_and_white == null ? "" : issue.black_and_white ? "yes" : "no",
    manga: ((): FormState["manga"] => {
      if (
        issue.manga === "Yes" ||
        issue.manga === "YesAndRightToLeft" ||
        issue.manga === "No"
      ) {
        return issue.manga;
      }
      return "";
    })(),
    sort_number: issue.sort_number != null ? String(issue.sort_number) : "",
    web_url: issue.web_url ?? "",
    gtin: issue.gtin ?? "",
    comicvine_id: issue.comicvine_id != null ? String(issue.comicvine_id) : "",
    metron_id: issue.metron_id != null ? String(issue.metron_id) : "",
    series_status: seriesStatus ?? "",
  };
}

function EditForm({
  issue,
  series,
  onSaved,
}: {
  issue: IssueDetailView;
  series: SeriesView | null;
  onSaved: () => void;
}) {
  const router = useRouter();
  const update = useUpdateIssue(issue.series_slug, issue.slug);
  // The series PATCH still works with `slug` even though the param is named
  // `seriesId` — the route is `/series/{slug}` and the hook just inserts the
  // value verbatim. The mutation is only created when we have the series.
  const updateSeries = useUpdateSeries(issue.series_slug);

  const [form, setForm] = useState<FormState>(() =>
    initialState(issue, series?.status ?? null),
  );
  const [links, setLinks] = useState<FormLink[]>(
    issue.additional_links.map((l) => ({
      label: l.label ?? "",
      url: l.url,
    })),
  );

  const set = <K extends keyof FormState>(key: K, value: FormState[K]) =>
    setForm((s) => ({ ...s, [key]: value }));

  const onLinkChange = (i: number, field: keyof FormLink, value: string) => {
    setLinks((arr) =>
      arr.map((l, idx) => (idx === i ? { ...l, [field]: value } : l)),
    );
  };
  const onAddLink = () => setLinks((arr) => [...arr, { label: "", url: "" }]);
  const onRemoveLink = (i: number) =>
    setLinks((arr) => arr.filter((_, idx) => idx !== i));

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const issueBody = buildPatchBody(issue, form, links);
    const seriesBody = buildSeriesPatchBody(series, form);

    const issueDirty = Object.keys(issueBody).length > 0;
    const seriesDirty = Object.keys(seriesBody).length > 0;

    if (!issueDirty && !seriesDirty) {
      onSaved();
      return;
    }

    // Fan-out: issue and series PATCHes are independent so we run them
    // in parallel. Toasts come from the mutations themselves; we only
    // need to refresh + close once everything settles.
    const tasks: Promise<unknown>[] = [];
    if (issueDirty) tasks.push(update.mutateAsync(issueBody));
    if (seriesDirty) tasks.push(updateSeries.mutateAsync(seriesBody));
    void Promise.all(tasks)
      .then(() => {
        router.refresh();
        onSaved();
      })
      .catch(() => {
        /* error toast already raised by useApiMutation */
      });
  };
  const isPending = update.isPending || updateSeries.isPending;

  return (
    <form onSubmit={onSubmit} className="flex min-h-0 flex-1 flex-col">
      <div className="flex-1 overflow-y-auto px-6 py-5">
        <div className="space-y-8">
          <Section title="Identity" hint="Title and issue ordering.">
            <Field label="Title" htmlFor="ed-title">
              <Input
                id="ed-title"
                value={form.title}
                onChange={(e) => set("title", e.target.value)}
                placeholder="Issue title (e.g. The Beginning)"
              />
            </Field>
            <Row>
              <Field label="Issue number" htmlFor="ed-number">
                <Input
                  id="ed-number"
                  value={form.number}
                  onChange={(e) => set("number", e.target.value)}
                  placeholder="1, 1.5, Annual 2"
                />
              </Field>
              <Field label="Sort number" htmlFor="ed-sort">
                <Input
                  id="ed-sort"
                  type="number"
                  step="0.0001"
                  value={form.sort_number}
                  onChange={(e) => set("sort_number", e.target.value)}
                  placeholder="Used for ordering within a series"
                />
              </Field>
            </Row>
            <Row>
              <Field label="Volume" htmlFor="ed-volume">
                <Input
                  id="ed-volume"
                  type="number"
                  min={0}
                  max={9999}
                  value={form.volume}
                  onChange={(e) => set("volume", e.target.value)}
                />
              </Field>
              <Field label="Alternate series" htmlFor="ed-alt-series">
                <Input
                  id="ed-alt-series"
                  value={form.alternate_series}
                  onChange={(e) => set("alternate_series", e.target.value)}
                  placeholder="Crossover or reprint name"
                />
              </Field>
            </Row>
            <Field label="Summary" htmlFor="ed-summary">
              <Textarea
                id="ed-summary"
                rows={4}
                value={form.summary}
                onChange={(e) => set("summary", e.target.value)}
                placeholder="Short synopsis shown above the metadata."
              />
            </Field>
            <Field label="Notes" htmlFor="ed-notes">
              <Textarea
                id="ed-notes"
                rows={3}
                value={form.notes}
                onChange={(e) => set("notes", e.target.value)}
                placeholder="Free-form notes from ComicInfo or your own."
              />
            </Field>
          </Section>

          <Section
            title="Publication"
            hint="Publisher and date the issue was released."
          >
            <Row>
              <Field label="Publisher" htmlFor="ed-publisher">
                <Input
                  id="ed-publisher"
                  value={form.publisher}
                  onChange={(e) => set("publisher", e.target.value)}
                />
              </Field>
              <Field label="Imprint" htmlFor="ed-imprint">
                <Input
                  id="ed-imprint"
                  value={form.imprint}
                  onChange={(e) => set("imprint", e.target.value)}
                />
              </Field>
            </Row>
            <Row3>
              <Field label="Year" htmlFor="ed-year">
                <Input
                  id="ed-year"
                  type="number"
                  min={1800}
                  max={2999}
                  value={form.year}
                  onChange={(e) => set("year", e.target.value)}
                />
              </Field>
              <Field label="Month" htmlFor="ed-month">
                <Input
                  id="ed-month"
                  type="number"
                  min={1}
                  max={12}
                  value={form.month}
                  onChange={(e) => set("month", e.target.value)}
                />
              </Field>
              <Field label="Day" htmlFor="ed-day">
                <Input
                  id="ed-day"
                  type="number"
                  min={1}
                  max={31}
                  value={form.day}
                  onChange={(e) => set("day", e.target.value)}
                />
              </Field>
            </Row3>
          </Section>

          <Section title="Credits" hint="Comma- or semicolon-separated names.">
            <Row>
              <Field label="Writer" htmlFor="ed-writer">
                <Input
                  id="ed-writer"
                  value={form.writer}
                  onChange={(e) => set("writer", e.target.value)}
                />
              </Field>
              <Field label="Penciller" htmlFor="ed-penciller">
                <Input
                  id="ed-penciller"
                  value={form.penciller}
                  onChange={(e) => set("penciller", e.target.value)}
                />
              </Field>
            </Row>
            <Row>
              <Field label="Inker" htmlFor="ed-inker">
                <Input
                  id="ed-inker"
                  value={form.inker}
                  onChange={(e) => set("inker", e.target.value)}
                />
              </Field>
              <Field label="Colorist" htmlFor="ed-colorist">
                <Input
                  id="ed-colorist"
                  value={form.colorist}
                  onChange={(e) => set("colorist", e.target.value)}
                />
              </Field>
            </Row>
            <Row>
              <Field label="Letterer" htmlFor="ed-letterer">
                <Input
                  id="ed-letterer"
                  value={form.letterer}
                  onChange={(e) => set("letterer", e.target.value)}
                />
              </Field>
              <Field label="Cover artist" htmlFor="ed-cover-artist">
                <Input
                  id="ed-cover-artist"
                  value={form.cover_artist}
                  onChange={(e) => set("cover_artist", e.target.value)}
                />
              </Field>
            </Row>
            <Row>
              <Field label="Editor" htmlFor="ed-editor">
                <Input
                  id="ed-editor"
                  value={form.editor}
                  onChange={(e) => set("editor", e.target.value)}
                />
              </Field>
              <Field label="Translator" htmlFor="ed-translator">
                <Input
                  id="ed-translator"
                  value={form.translator}
                  onChange={(e) => set("translator", e.target.value)}
                />
              </Field>
            </Row>
          </Section>

          <Section
            title="Cast & setting"
            hint="Story arc plus the people, teams, and places featured."
          >
            <Field label="Characters" htmlFor="ed-characters">
              <Input
                id="ed-characters"
                value={form.characters}
                onChange={(e) => set("characters", e.target.value)}
                placeholder="Spider-Man, Mary Jane, …"
              />
            </Field>
            <Row>
              <Field label="Teams" htmlFor="ed-teams">
                <Input
                  id="ed-teams"
                  value={form.teams}
                  onChange={(e) => set("teams", e.target.value)}
                />
              </Field>
              <Field label="Locations" htmlFor="ed-locations">
                <Input
                  id="ed-locations"
                  value={form.locations}
                  onChange={(e) => set("locations", e.target.value)}
                />
              </Field>
            </Row>
            <Row>
              <Field label="Story arc" htmlFor="ed-story-arc">
                <Input
                  id="ed-story-arc"
                  value={form.story_arc}
                  onChange={(e) => set("story_arc", e.target.value)}
                />
              </Field>
              <Field label="Story arc number" htmlFor="ed-story-arc-number">
                <Input
                  id="ed-story-arc-number"
                  value={form.story_arc_number}
                  onChange={(e) => set("story_arc_number", e.target.value)}
                  placeholder="1, 2, …"
                />
              </Field>
            </Row>
          </Section>

          <Section
            title="Classification"
            hint="Genre, tags, language, format, and age rating."
          >
            <Row>
              <Field label="Genre" htmlFor="ed-genre">
                <Input
                  id="ed-genre"
                  value={form.genre}
                  onChange={(e) => set("genre", e.target.value)}
                  placeholder="Action, Sci-Fi"
                />
              </Field>
              <Field label="Tags" htmlFor="ed-tags">
                <Input
                  id="ed-tags"
                  value={form.tags}
                  onChange={(e) => set("tags", e.target.value)}
                  placeholder="space-marines, crossover, …"
                />
              </Field>
            </Row>
            <Row3>
              <Field label="Language" htmlFor="ed-lang">
                <Input
                  id="ed-lang"
                  value={form.language_code}
                  onChange={(e) => set("language_code", e.target.value)}
                  placeholder="en, fr, ja"
                  maxLength={16}
                />
              </Field>
              <Field label="Age rating" htmlFor="ed-age">
                <Input
                  id="ed-age"
                  value={form.age_rating}
                  onChange={(e) => set("age_rating", e.target.value)}
                  placeholder="Teen, Mature 17+"
                />
              </Field>
              <Field label="Format" htmlFor="ed-format">
                <Input
                  id="ed-format"
                  value={form.format}
                  onChange={(e) => set("format", e.target.value)}
                  placeholder="One-Shot, TPB, Annual"
                />
              </Field>
            </Row3>
            <Row>
              <Field label="Black & white" htmlFor="ed-bw">
                <NativeSelect
                  id="ed-bw"
                  value={form.black_and_white}
                  onChange={(v) =>
                    set("black_and_white", v as FormState["black_and_white"])
                  }
                  options={[
                    { value: "", label: "—" },
                    { value: "yes", label: "Yes" },
                    { value: "no", label: "No" },
                  ]}
                />
              </Field>
              <Field label="Manga" htmlFor="ed-manga">
                <NativeSelect
                  id="ed-manga"
                  value={form.manga}
                  onChange={(v) => set("manga", v as FormState["manga"])}
                  options={[
                    { value: "", label: "—" },
                    { value: "No", label: "No" },
                    { value: "Yes", label: "Yes (left-to-right)" },
                    {
                      value: "YesAndRightToLeft",
                      label: "Yes (right-to-left)",
                    },
                  ]}
                />
              </Field>
            </Row>
          </Section>

          {series && (
            <Section
              title="Series"
              hint="Series-wide fields. These apply to every issue in this series."
            >
              <Field label="Publication status" htmlFor="ed-series-status">
                <NativeSelect
                  id="ed-series-status"
                  value={form.series_status}
                  onChange={(v) => set("series_status", v)}
                  options={SERIES_STATUS_OPTIONS}
                />
              </Field>
            </Section>
          )}

          <Section
            title="External"
            hint="Web link, external-database IDs, and additional links."
          >
            <Row>
              <Field label="Web URL" htmlFor="ed-web-url">
                <Input
                  id="ed-web-url"
                  type="url"
                  value={form.web_url}
                  onChange={(e) => set("web_url", e.target.value)}
                  placeholder="https://…"
                />
              </Field>
              <Field label="GTIN" htmlFor="ed-gtin">
                <Input
                  id="ed-gtin"
                  value={form.gtin}
                  onChange={(e) => set("gtin", e.target.value)}
                  placeholder="ISBN / EAN / UPC"
                />
              </Field>
            </Row>
            <Row>
              <Field label="ComicVine issue ID" htmlFor="ed-comicvine">
                <Input
                  id="ed-comicvine"
                  type="number"
                  inputMode="numeric"
                  value={form.comicvine_id}
                  onChange={(e) => set("comicvine_id", e.target.value)}
                  placeholder="e.g. 381432"
                />
              </Field>
              <Field label="Metron issue ID" htmlFor="ed-metron">
                <Input
                  id="ed-metron"
                  type="number"
                  inputMode="numeric"
                  value={form.metron_id}
                  onChange={(e) => set("metron_id", e.target.value)}
                  placeholder="e.g. 12345"
                />
              </Field>
            </Row>
            <div className="grid gap-2">
              <div className="flex items-center justify-between">
                <Label>Additional links</Label>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={onAddLink}
                >
                  <Plus className="mr-1 h-4 w-4" />
                  Add link
                </Button>
              </div>
              {links.length === 0 ? (
                <p className="text-muted-foreground text-xs">
                  None. Click &ldquo;Add link&rdquo; to attach one.
                </p>
              ) : (
                <ul className="space-y-2">
                  {links.map((link, i) => (
                    <li key={i} className="grid grid-cols-[1fr_2fr_auto] gap-2">
                      <Input
                        placeholder="Label (optional)"
                        value={link.label}
                        onChange={(e) =>
                          onLinkChange(i, "label", e.target.value)
                        }
                      />
                      <Input
                        placeholder="https://…"
                        value={link.url}
                        onChange={(e) => onLinkChange(i, "url", e.target.value)}
                      />
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        onClick={() => onRemoveLink(i)}
                        aria-label="Remove link"
                      >
                        <Trash2 className="h-4 w-4" />
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </Section>
        </div>
      </div>

      <div className="border-border flex items-center justify-end gap-2 border-t px-6 py-4">
        <Button
          type="button"
          variant="ghost"
          onClick={onSaved}
          disabled={isPending}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={isPending}>
          {isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
          Save
        </Button>
      </div>
    </form>
  );
}

// ───── form layout helpers ─────

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

function Row({ children }: { children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</div>
  );
}

function Row3({ children }: { children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">{children}</div>
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

// ───── diff helpers ─────

/**
 * Build the PATCH body containing only fields the user actually changed.
 * Empty strings collapse to `null` so the user can clear values.
 */
function buildPatchBody(
  prev: IssueDetailView,
  form: FormState,
  links: FormLink[],
): UpdateIssueReq {
  const body: UpdateIssueReq = {};

  // String fields: trim, then null-on-empty, send only if changed.
  const str = (
    field: keyof UpdateIssueReq,
    next: string,
    prevValue: string | null,
  ) => {
    if (didChangeStr(prevValue, next)) {
      (body as Record<string, unknown>)[field] = emptyToNull(next);
    }
  };

  str("title", form.title, prev.title);
  str("number", form.number, prev.number);
  str("summary", form.summary, prev.summary);
  str("notes", form.notes, prev.notes);
  str("publisher", form.publisher, prev.publisher);
  str("imprint", form.imprint, prev.imprint);
  str("writer", form.writer, prev.writer);
  str("penciller", form.penciller, prev.penciller);
  str("inker", form.inker, prev.inker);
  str("colorist", form.colorist, prev.colorist);
  str("letterer", form.letterer, prev.letterer);
  str("cover_artist", form.cover_artist, prev.cover_artist);
  str("editor", form.editor, prev.editor);
  str("translator", form.translator, prev.translator);
  str("characters", form.characters, prev.characters);
  str("teams", form.teams, prev.teams);
  str("locations", form.locations, prev.locations);
  str("alternate_series", form.alternate_series, prev.alternate_series);
  str("story_arc", form.story_arc, prev.story_arc);
  str("story_arc_number", form.story_arc_number, prev.story_arc_number);
  str("genre", form.genre, prev.genre);
  str("tags", form.tags, prev.tags);
  str("language_code", form.language_code, prev.language_code);
  str("age_rating", form.age_rating, prev.age_rating);
  str("format", form.format, prev.format);
  str("web_url", form.web_url, prev.web_url);
  str("gtin", form.gtin, prev.gtin);
  str("manga", form.manga, prev.manga);

  // Integer fields.
  if (didChangeInt(prev.volume, form.volume)) {
    body.volume = parseIntOrNull(form.volume);
  }
  if (didChangeInt(prev.year, form.year)) {
    body.year = parseIntOrNull(form.year);
  }
  if (didChangeInt(prev.month, form.month)) {
    body.month = parseIntOrNull(form.month);
  }
  if (didChangeInt(prev.day, form.day)) {
    body.day = parseIntOrNull(form.day);
  }

  // Float field.
  if (didChangeNum(prev.sort_number, form.sort_number)) {
    body.sort_number = parseSortNumber(form.sort_number);
  }

  // External-database IDs (i64). Empty string → null.
  if (didChangeInt(prev.comicvine_id, form.comicvine_id)) {
    body.comicvine_id = parseIntOrNull(form.comicvine_id);
  }
  if (didChangeInt(prev.metron_id, form.metron_id)) {
    body.metron_id = parseIntOrNull(form.metron_id);
  }

  // Tri-state black & white ("" | "yes" | "no") → bool | null.
  const nextBw =
    form.black_and_white === "" ? null : form.black_and_white === "yes";
  if (nextBw !== prev.black_and_white) {
    body.black_and_white = nextBw;
  }

  if (linksChanged(prev.additional_links, links)) {
    body.additional_links = links
      .filter((l) => l.url.trim() !== "")
      .map<IssueLink>((l) => ({
        label: l.label.trim() || null,
        url: l.url.trim(),
      }));
  }

  return body;
}

/**
 * Build the PATCH body for the parent series. Only `status` is exposed in
 * the issue drawer today; the rest of the series fields live on the (not
 * yet built) series settings page. Returns `{}` when nothing changed.
 */
function buildSeriesPatchBody(
  series: SeriesView | null,
  form: FormState,
): UpdateSeriesReq {
  const body: UpdateSeriesReq = {};
  if (!series) return body;
  const next = form.series_status.trim().toLowerCase();
  // Empty selection means "leave as-is" — the field is required server-side
  // (NOT NULL), so we never send "" to clear it. The "—" choice is just a
  // placeholder for "no change."
  if (next !== "" && next !== series.status.toLowerCase()) {
    body.status = next;
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

function didChangeNum(prev: number | null, next: string): boolean {
  const trimmed = next.trim();
  if (trimmed === "") return prev != null;
  const n = Number(trimmed);
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

function parseSortNumber(value: string): number | null {
  const t = value.trim();
  if (t === "") return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

function linksChanged(prev: IssueLink[], next: FormLink[]): boolean {
  const sanitized = next
    .filter((l) => l.url.trim() !== "")
    .map((l) => ({ label: l.label.trim() || null, url: l.url.trim() }));
  if (sanitized.length !== prev.length) return true;
  for (let i = 0; i < sanitized.length; i++) {
    const a = sanitized[i]!;
    const b = prev[i]!;
    if (a.url !== b.url) return true;
    if ((a.label ?? "") !== (b.label ?? "")) return true;
  }
  return false;
}
