"use client";

import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Skeleton } from "@/components/ui/skeleton";
import { TagInput } from "./TagInput";
import { CronInput } from "./CronInput";
import { useLibrary, useThumbnailsSettings } from "@/lib/api/queries";
import {
  useUpdateLibrary,
  useUpdateThumbnailsSettings,
} from "@/lib/api/mutations";
import type { ThumbnailFormat } from "@/lib/api/types";
import { validateCron } from "@/lib/api/cron";
import { cn } from "@/lib/utils";

const THUMBNAIL_FORMATS: {
  value: ThumbnailFormat;
  label: string;
  hint: string;
}[] = [
  { value: "webp", label: "WebP", hint: "Smallest files, modern browsers" },
  { value: "jpeg", label: "JPEG", hint: "Universal compatibility, lossy" },
  { value: "png", label: "PNG", hint: "Lossless, larger files" },
];

const schema = z.object({
  ignore_globs: z.array(z.string().min(1)).default([]),
  scan_schedule_cron: z
    .string()
    .refine((v) => validateCron(v).ok, "Invalid cron expression")
    .default(""),
  report_missing_comicinfo: z.boolean().default(false),
  soft_delete_days: z.number().int().min(0).max(365).default(7),
  generate_page_thumbs_on_scan: z.boolean().default(false),
});

type FormValues = z.infer<typeof schema>;

export function LibrarySettingsForm({ id }: { id: string }) {
  const lib = useLibrary(id);
  const thumbnailSettings = useThumbnailsSettings(id);
  const update = useUpdateLibrary(id);
  const updateThumbnailSettings = useUpdateThumbnailsSettings(id);
  const form = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: {
      ignore_globs: [],
      scan_schedule_cron: "",
      report_missing_comicinfo: false,
      soft_delete_days: 7,
      generate_page_thumbs_on_scan: false,
    },
  });

  useEffect(() => {
    if (lib.data) {
      form.reset({
        ignore_globs: lib.data.ignore_globs,
        scan_schedule_cron: lib.data.scan_schedule_cron ?? "",
        report_missing_comicinfo: lib.data.report_missing_comicinfo,
        soft_delete_days: lib.data.soft_delete_days,
        generate_page_thumbs_on_scan: lib.data.generate_page_thumbs_on_scan,
      });
    }
  }, [lib.data, form]);

  if (lib.isLoading) return <Skeleton className="h-72 w-full" />;
  if (lib.error || !lib.data) {
    return <p className="text-destructive text-sm">Failed to load library.</p>;
  }

  const onSubmit = form.handleSubmit((values) => {
    update.mutate({
      ignore_globs: values.ignore_globs,
      report_missing_comicinfo: values.report_missing_comicinfo,
      soft_delete_days: values.soft_delete_days,
      generate_page_thumbs_on_scan: values.generate_page_thumbs_on_scan,
      scan_schedule_cron:
        values.scan_schedule_cron.trim() === ""
          ? null
          : values.scan_schedule_cron.trim(),
    });
  });

  return (
    <Form {...form}>
      <form onSubmit={onSubmit} className="space-y-6">
        <Card>
          <CardContent className="space-y-5 p-6">
            <FormField
              control={form.control}
              name="ignore_globs"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>Ignore globs</FormLabel>
                  <FormDescription>
                    Glob patterns the scanner skips. Press Enter or comma to
                    add. Example: <span className="font-mono">**/.tmp/*</span>
                  </FormDescription>
                  <FormControl>
                    <TagInput value={field.value} onChange={field.onChange} />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="scan_schedule_cron"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>Scan schedule</FormLabel>
                  <FormDescription>
                    Cron expression. Leave empty to disable scheduled scans.
                  </FormDescription>
                  <FormControl>
                    <CronInput value={field.value} onChange={field.onChange} />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>
        <Card>
          <CardContent className="space-y-5 p-6">
            <FormField
              control={form.control}
              name="report_missing_comicinfo"
              render={({ field }) => (
                <FormItem className="flex items-start justify-between gap-6">
                  <div className="space-y-1">
                    <FormLabel>Report missing ComicInfo</FormLabel>
                    <FormDescription>
                      Surface a health issue when an issue lacks ComicInfo
                      metadata.
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="generate_page_thumbs_on_scan"
              render={({ field }) => (
                <FormItem className="flex items-start justify-between gap-6">
                  <div className="space-y-1">
                    <FormLabel>Auto-generate page thumbnails on scan</FormLabel>
                    <FormDescription>
                      Cover thumbnails are always generated. When this is on,
                      the post-scan pipeline also enqueues per-page strip
                      thumbnails (pricier — one image per page). Off by default;
                      you can fill in missing page thumbnails on demand from the
                      Thumbnails tab.
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="soft_delete_days"
              render={({ field }) => (
                <FormItem>
                  <div className="flex items-end justify-between gap-3">
                    <FormLabel>Soft-delete window</FormLabel>
                    <span className="text-muted-foreground text-sm">
                      {field.value === 0
                        ? "Confirm immediately"
                        : `${field.value} day${field.value === 1 ? "" : "s"}`}
                    </span>
                  </div>
                  <FormControl>
                    <Slider
                      min={0}
                      max={365}
                      step={1}
                      value={[field.value]}
                      onValueChange={(v) => field.onChange(v[0])}
                    />
                  </FormControl>
                  <FormDescription>
                    Days a removed file stays soft-deleted before the operator
                    must confirm permanent removal.
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>
        <ThumbnailSettingsCard
          enabled={thumbnailSettings.data?.enabled ?? true}
          format={thumbnailSettings.data?.format ?? "webp"}
          coverQuality={thumbnailSettings.data?.cover_quality ?? 80}
          pageQuality={thumbnailSettings.data?.page_quality ?? 50}
          loading={thumbnailSettings.isLoading}
          disabled={updateThumbnailSettings.isPending}
          onEnabledChange={(enabled) =>
            updateThumbnailSettings.mutate({ enabled })
          }
          onFormatChange={(format) =>
            updateThumbnailSettings.mutate({ format })
          }
          onCoverQualityChange={(cover_quality) =>
            updateThumbnailSettings.mutate({ cover_quality })
          }
          onPageQualityChange={(page_quality) =>
            updateThumbnailSettings.mutate({ page_quality })
          }
        />
        <div className="flex items-center justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            disabled={update.isPending}
            onClick={() =>
              lib.data &&
              form.reset({
                ignore_globs: lib.data.ignore_globs,
                scan_schedule_cron: lib.data.scan_schedule_cron ?? "",
                report_missing_comicinfo: lib.data.report_missing_comicinfo,
                soft_delete_days: lib.data.soft_delete_days,
              })
            }
          >
            Reset
          </Button>
          <Button
            type="submit"
            disabled={update.isPending || !form.formState.isDirty}
          >
            {update.isPending ? "Saving…" : "Save changes"}
          </Button>
        </div>
      </form>
    </Form>
  );
}

function ThumbnailSettingsCard({
  enabled,
  format,
  coverQuality,
  pageQuality,
  loading,
  disabled,
  onEnabledChange,
  onFormatChange,
  onCoverQualityChange,
  onPageQualityChange,
}: {
  enabled: boolean;
  format: ThumbnailFormat;
  coverQuality: number;
  pageQuality: number;
  loading: boolean;
  disabled: boolean;
  onEnabledChange: (enabled: boolean) => void;
  onFormatChange: (format: ThumbnailFormat) => void;
  onCoverQualityChange: (quality: number) => void;
  onPageQualityChange: (quality: number) => void;
}) {
  if (loading) return <Skeleton className="h-48 w-full" />;

  return (
    <Card>
      <CardContent className="space-y-5 p-6">
        <div>
          <h3 className="text-foreground text-sm font-semibold tracking-tight">
            Thumbnail settings
          </h3>
          <p className="text-muted-foreground mt-1 text-xs">
            Configure how thumbnail jobs operate. Generation actions and queue
            status live on the Live scan page.
          </p>
        </div>

        <div className="flex items-start justify-between gap-6">
          <div className="space-y-1">
            <p className="text-foreground text-sm font-medium">
              Generate thumbnails
            </p>
            <p className="text-muted-foreground text-sm">
              When off, scans skip thumbnail generation. Existing thumbnails
              continue serving from disk.
            </p>
          </div>
          <Switch
            checked={enabled}
            disabled={disabled}
            onCheckedChange={onEnabledChange}
          />
        </div>

        <div className="space-y-2">
          <p className="text-foreground text-sm font-medium">Image format</p>
          <div
            role="radiogroup"
            aria-label="Thumbnail image format"
            className="grid gap-2 sm:grid-cols-3"
          >
            {THUMBNAIL_FORMATS.map((option) => {
              const active = format === option.value;
              return (
                <button
                  key={option.value}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  disabled={disabled || !enabled}
                  onClick={() => {
                    if (format !== option.value) onFormatChange(option.value);
                  }}
                  className={cn(
                    "rounded-md border px-3 py-2 text-left text-sm transition-colors",
                    active
                      ? "border-primary bg-primary/5 text-foreground"
                      : "border-border bg-background text-muted-foreground hover:text-foreground",
                    (disabled || !enabled) && "opacity-60",
                  )}
                >
                  <div className="font-medium tracking-wide uppercase">
                    {option.label}
                  </div>
                  <div className="text-muted-foreground text-xs">
                    {option.hint}
                  </div>
                </button>
              );
            })}
          </div>
        </div>

        <div className="grid gap-5 md:grid-cols-2">
          <ThumbnailQualitySlider
            label="Cover quality"
            description="Controls generated cover thumbnail quality for WebP/JPEG."
            value={coverQuality}
            disabled={disabled || !enabled}
            onCommit={onCoverQualityChange}
          />
          <ThumbnailQualitySlider
            label="Page thumbnail quality"
            description="Controls generated reader page thumbnail quality for WebP/JPEG."
            value={pageQuality}
            disabled={disabled || !enabled}
            onCommit={onPageQualityChange}
          />
        </div>
        {format === "png" ? (
          <p className="text-muted-foreground text-xs">
            PNG thumbnails remain lossless; quality settings apply when using
            WebP or JPEG.
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}

function ThumbnailQualitySlider({
  label,
  description,
  value,
  disabled,
  onCommit,
}: {
  label: string;
  description: string;
  value: number;
  disabled: boolean;
  onCommit: (value: number) => void;
}) {
  const [draft, setDraft] = useState(value);

  // Resync the draft when the parent's committed value changes —
  // typically after a successful PATCH or after another tab updates
  // the same row. The lint rule flags setState-in-effect, but here
  // it's the canonical sync-from-prop pattern.
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setDraft(value);
  }, [value]);

  return (
    <div className="space-y-2">
      <div className="flex items-end justify-between gap-3">
        <div>
          <p className="text-foreground text-sm font-medium">{label}</p>
          <p className="text-muted-foreground text-xs">{description}</p>
        </div>
        <span className="text-muted-foreground text-sm tabular-nums">
          {draft}
        </span>
      </div>
      <Slider
        min={0}
        max={100}
        step={1}
        value={[draft]}
        disabled={disabled}
        onValueChange={(next) => setDraft(next[0] ?? 0)}
        onValueCommit={(next) => {
          const committed = next[0] ?? value;
          if (committed !== value) onCommit(committed);
        }}
      />
      <div className="text-muted-foreground flex justify-between text-[11px]">
        <span>Smaller</span>
        <span>Sharper</span>
      </div>
    </div>
  );
}
