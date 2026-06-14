"use client";

import * as React from "react";

import { Button } from "@/components/ui/button";
import { FilterPill } from "@/components/ui/filter-pill";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { usePatchLogWidget } from "@/lib/api/mutations";
import type {
  CreatorRole,
  LogWidgetKind,
  LogWidgetView,
  ReadingLogEventKind,
  ReadingStatsRange,
} from "@/lib/api/types";

import { WIDGET_REGISTRY } from "./widgets";
import type {
  ChronoFeedConfig,
  CurrentlyReadingConfig,
  HeatmapConfig,
  NoteConfig,
  PaceChartConfig,
  RankingConfig,
  RecentBookmarksConfig,
  StatsHeroConfig,
  StatsHeroMetric,
  TopCreatorsConfig,
} from "./widgets/types";

const RANGE_OPTS: ReadonlyArray<{ value: ReadingStatsRange; label: string }> = [
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "60d", label: "60 days" },
  { value: "90d", label: "90 days" },
  { value: "1y", label: "1 year" },
  { value: "all", label: "All time" },
];

const CHRONO_GROUP_OPTS = [
  { value: "day", label: "By day" },
  { value: "week", label: "By week" },
  { value: "month", label: "By month" },
  { value: "none", label: "Flat list" },
] as const;

const CREATOR_ROLES: ReadonlyArray<{ value: CreatorRole; label: string }> = [
  { value: "writer", label: "Writer" },
  { value: "penciller", label: "Penciller" },
  { value: "inker", label: "Inker" },
  { value: "colorist", label: "Colorist" },
  { value: "letterer", label: "Letterer" },
  { value: "cover_artist", label: "Cover artist" },
  { value: "editor", label: "Editor" },
  { value: "translator", label: "Translator" },
];

const EVENT_KINDS: ReadonlyArray<{
  value: ReadingLogEventKind;
  label: string;
}> = [
  { value: "issue_finished", label: "Issues finished" },
  { value: "series_finished", label: "Series finished" },
  { value: "session_completed", label: "Sessions" },
  { value: "marker_created", label: "Markers" },
];

const STATS_METRICS: ReadonlyArray<{
  value: StatsHeroMetric;
  label: string;
}> = [
  { value: "issues", label: "Issues read" },
  { value: "hours", label: "Time read" },
  { value: "streak", label: "Current streak" },
  { value: "pages", label: "Pages read" },
  { value: "pace_spp", label: "Seconds/page" },
];

const MARKER_KINDS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "bookmark", label: "Bookmark" },
  { value: "note", label: "Note" },
  { value: "highlight", label: "Highlight" },
  { value: "favorite", label: "Favorite" },
];

const HEATMAP_WEEKS = [4, 8, 12, 26, 52] as const;

/** Per-widget configuration dialog. Dispatches on `widget.kind` to
 *  the matching form body; collects the form's draft config in
 *  local state and PATCHes on Save. Cancel discards. */
export function ConfigureWidgetDialog({
  widget,
  open,
  onOpenChange,
}: {
  widget: LogWidgetView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const def = WIDGET_REGISTRY[widget.kind as LogWidgetKind];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Configure {def?.displayName ?? widget.kind}</DialogTitle>
          {def?.description ? (
            <DialogDescription>{def.description}</DialogDescription>
          ) : null}
        </DialogHeader>
        {/* Body lives in a child so its draft `useState` initializer
         *  runs fresh on every open — Radix Dialog unmounts the
         *  content on close, so a Cancel-then-reopen cycle starts
         *  from the latest server config without an explicit
         *  reset-on-open effect (which would trip
         *  `react-hooks/set-state-in-effect`). */}
        <ConfigureFormBody
          widget={widget}
          onClose={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}

function ConfigureFormBody({
  widget,
  onClose,
}: {
  widget: LogWidgetView;
  onClose: () => void;
}) {
  const patch = usePatchLogWidget(widget.id);
  const [draft, setDraft] = React.useState<Record<string, unknown>>(
    (widget.config as Record<string, unknown> | null | undefined) ?? {},
  );
  const onSave = () => {
    patch.mutate({ config: draft }, { onSuccess: () => onClose() });
  };
  return (
    <>
      <ConfigureBody
        kind={widget.kind as LogWidgetKind}
        draft={draft}
        setDraft={setDraft}
      />
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <Button onClick={onSave} disabled={patch.isPending}>
          {patch.isPending ? "Saving…" : "Save"}
        </Button>
      </DialogFooter>
    </>
  );
}

function ConfigureBody({
  kind,
  draft,
  setDraft,
}: {
  kind: LogWidgetKind;
  draft: Record<string, unknown>;
  setDraft: React.Dispatch<React.SetStateAction<Record<string, unknown>>>;
}) {
  const set = <K extends string>(key: K, value: unknown) =>
    setDraft((d) => ({ ...d, [key]: value }));
  switch (kind) {
    case "chrono_feed": {
      const c = draft as Partial<ChronoFeedConfig>;
      return (
        <div className="space-y-4">
          <FieldRow
            label="Width"
            description="Wide spans both columns; narrow lets you pin another widget next to the feed."
          >
            <Select
              value={c.size ?? "full"}
              onValueChange={(v) => set("size", v)}
            >
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="full">Wide</SelectItem>
                <SelectItem value="half">Narrow</SelectItem>
              </SelectContent>
            </Select>
          </FieldRow>
          <FieldRow
            label="Group"
            description="How events are bucketed. Same-series finishes within a group collapse into one row."
          >
            <Select
              value={c.group_by ?? "day"}
              onValueChange={(v) => set("group_by", v)}
            >
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {CHRONO_GROUP_OPTS.map((o) => (
                  <SelectItem key={o.value} value={o.value}>
                    {o.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </FieldRow>
          <FieldRow
            label="Range"
            description="Empty follows the page-level range selector."
          >
            <Select
              value={c.range && c.range.length > 0 ? c.range : "__page"}
              onValueChange={(v) => set("range", v === "__page" ? "" : v)}
            >
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__page">Page default</SelectItem>
                {RANGE_OPTS.map((r) => (
                  <SelectItem key={r.value} value={r.value}>
                    {r.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </FieldRow>
          <ChipMulti
            label="Event kinds"
            description="Empty = all four kinds. Replaces the page's old chip row."
            options={EVENT_KINDS}
            value={c.default_kinds ?? []}
            onChange={(next) => set("default_kinds", next)}
          />
        </div>
      );
    }
    case "stats_hero": {
      const c = draft as Partial<StatsHeroConfig>;
      return (
        <ChipMulti
          label="Metrics to show"
          description="Pick up to five. Empty falls back to issues / hours / streak."
          options={STATS_METRICS}
          value={c.metrics ?? []}
          onChange={(next) =>
            set("metrics", (next as StatsHeroMetric[]).slice(0, 5))
          }
        />
      );
    }
    case "heatmap": {
      const c = draft as Partial<HeatmapConfig>;
      return (
        <FieldRow label="Window" description="Number of recent weeks to show.">
          <Select
            value={String(c.weeks ?? 52)}
            onValueChange={(v) => set("weeks", Number(v))}
          >
            <SelectTrigger className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {HEATMAP_WEEKS.map((w) => (
                <SelectItem key={w} value={String(w)}>
                  {w} weeks
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </FieldRow>
      );
    }
    case "top_creators": {
      const c = draft as Partial<TopCreatorsConfig>;
      return (
        <div className="space-y-4">
          <FieldRow label="Role">
            <Select
              value={c.role ?? "writer"}
              onValueChange={(v) => set("role", v)}
            >
              <SelectTrigger className="w-40">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {CREATOR_ROLES.map((r) => (
                  <SelectItem key={r.value} value={r.value}>
                    {r.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </FieldRow>
          <RangeField
            value={c.range ?? "30d"}
            onChange={(v) => set("range", v)}
          />
          <LimitField
            value={c.limit ?? 5}
            onChange={(v) => set("limit", v)}
            max={20}
          />
        </div>
      );
    }
    case "top_publishers":
    case "top_imprints":
    case "series_finishes": {
      const c = draft as Partial<RankingConfig>;
      return (
        <div className="space-y-4">
          <RangeField
            value={c.range ?? "30d"}
            onChange={(v) => set("range", v)}
          />
          <LimitField
            value={c.limit ?? 5}
            onChange={(v) => set("limit", v)}
            max={20}
          />
        </div>
      );
    }
    case "pace_chart": {
      const c = draft as Partial<PaceChartConfig>;
      return (
        <RangeField
          value={c.range ?? "30d"}
          onChange={(v) => set("range", v)}
        />
      );
    }
    case "time_of_day": {
      return (
        <p className="text-muted-foreground text-sm">
          This widget follows the page-level range and has no settings of its
          own.
        </p>
      );
    }
    case "recent_bookmarks": {
      const c = draft as Partial<RecentBookmarksConfig>;
      return (
        <div className="space-y-4">
          <LimitField
            value={c.limit ?? 5}
            onChange={(v) => set("limit", v)}
            max={20}
          />
          <ChipMulti
            label="Marker kinds"
            description="Empty = all kinds."
            options={MARKER_KINDS}
            value={c.kinds ?? []}
            onChange={(next) => set("kinds", next)}
          />
        </div>
      );
    }
    case "currently_reading": {
      const c = draft as Partial<CurrentlyReadingConfig>;
      return (
        <LimitField
          value={c.limit ?? 5}
          onChange={(v) => set("limit", v)}
          max={20}
        />
      );
    }
    case "note": {
      const c = draft as Partial<NoteConfig>;
      return (
        <div className="space-y-2">
          <Label htmlFor="note-body">Body</Label>
          <Textarea
            id="note-body"
            value={c.body ?? ""}
            onChange={(e) => set("body", e.target.value)}
            rows={6}
            placeholder="Pin a reading goal, a reminder, the current arc you're chasing…"
          />
          <p className="text-muted-foreground text-xs">
            Plain text — line breaks are preserved.
          </p>
        </div>
      );
    }
  }
}

function FieldRow({
  label,
  description,
  children,
}: {
  label: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-3">
      <div className="flex-1">
        <Label className="text-sm font-medium">{label}</Label>
        {description ? (
          <p className="text-muted-foreground text-xs">{description}</p>
        ) : null}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

function RangeField({
  value,
  onChange,
}: {
  value: ReadingStatsRange;
  onChange: (next: ReadingStatsRange) => void;
}) {
  return (
    <FieldRow label="Range">
      <Select
        value={value}
        onValueChange={(v) => onChange(v as ReadingStatsRange)}
      >
        <SelectTrigger className="w-32">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {RANGE_OPTS.map((r) => (
            <SelectItem key={r.value} value={r.value}>
              {r.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </FieldRow>
  );
}

function LimitField({
  value,
  onChange,
  max,
}: {
  value: number;
  onChange: (next: number) => void;
  max: number;
}) {
  return (
    <FieldRow label="Items" description={`1 to ${max}.`}>
      <Input
        type="number"
        min={1}
        max={max}
        value={value}
        onChange={(e) => {
          const n = Number(e.target.value);
          if (Number.isFinite(n)) onChange(Math.max(1, Math.min(max, n)));
        }}
        className="w-20"
      />
    </FieldRow>
  );
}

function ChipMulti<T extends { value: string; label: string }>({
  label,
  description,
  options,
  value,
  onChange,
}: {
  label: string;
  description?: string;
  options: ReadonlyArray<T>;
  value: string[];
  onChange: (next: string[]) => void;
}) {
  const toggle = (v: string) => {
    onChange(value.includes(v) ? value.filter((x) => x !== v) : [...value, v]);
  };
  return (
    <div className="space-y-2">
      <div>
        <Label className="text-sm font-medium">{label}</Label>
        {description ? (
          <p className="text-muted-foreground text-xs">{description}</p>
        ) : null}
      </div>
      <div className="flex flex-wrap gap-1.5">
        {options.map((o) => (
          <FilterPill
            key={o.value}
            active={value.includes(o.value)}
            onClick={() => toggle(o.value)}
          >
            {o.label}
          </FilterPill>
        ))}
      </div>
    </div>
  );
}
