"use client";

/**
 * "Adjust query" + "Paste provider URL" tools for the metadata-match
 * dialog (roadmap WP-2.8 / audit UX-13).
 *
 * Both paths produce a *new* search run and hand its id back through
 * `onNewRun`; the dialog adopts it and its existing candidate polling +
 * preview + apply flow takes over unchanged:
 *
 *   - **Adjust query** re-runs the provider search with per-run
 *     overrides (`{ name?, year?, publisher?, issue_number? }`). Only the
 *     fields that differ from what the current run searched are sent, so
 *     the run's `overridden` flag means what it says. The local series /
 *     issue rows are never touched.
 *   - **Paste provider URL** skips matching entirely: the server fetches
 *     that exact ComicVine / Metron / GCD record and returns a completed run
 *     holding it as the single HIGH candidate.
 *
 * Server-side validation (422 + `error.details`) binds onto the inputs
 * via `applyServerErrors`; the URL parser is server-authoritative, so no
 * client-side parsing is duplicated here.
 */

import { ChevronRight, Link2 } from "lucide-react";
import * as React from "react";
import { useForm } from "react-hook-form";

import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { applyServerErrors } from "@/lib/api/form-errors";
import {
  useLookupMetadataForIssue,
  useLookupMetadataForSeries,
  useSearchMetadataForIssue,
  useSearchMetadataForSeries,
} from "@/lib/api/mutations";
import type { SearchOverrides, SearchQueryView } from "@/lib/api/types";
import type { MetadataMatchScope } from "@/components/library/metadata-match-scope";

type AdjustForm = {
  name: string;
  year: string;
  publisher: string;
  issue_number: string;
};

type LookupForm = { url: string };

/** Build the override body from the form, keeping only fields that
 *  differ from what the current run searched (`defaults`). Returns
 *  `null` when nothing changed so the caller can skip the request. */
export function overridesFromForm(
  values: AdjustForm,
  defaults: SearchQueryView | null,
  isIssue: boolean,
): SearchOverrides | null {
  const out: SearchOverrides = {};
  const name = values.name.trim();
  if (name && name !== (defaults?.name ?? "")) out.name = name;
  const year = values.year.trim();
  if (year) {
    const n = Number(year);
    if (Number.isFinite(n) && n !== (defaults?.year ?? null)) out.year = n;
  }
  const publisher = values.publisher.trim();
  if (publisher && publisher !== (defaults?.publisher ?? "")) {
    out.publisher = publisher;
  }
  if (isIssue) {
    const number = values.issue_number.trim();
    if (number && number !== (defaults?.issue_number ?? "")) {
      out.issue_number = number;
    }
  }
  return Object.keys(out).length === 0 ? null : out;
}

export function MetadataQueryTools({
  scope,
  defaults,
  disabled,
  onNewRun,
}: {
  scope: MetadataMatchScope;
  /** The current run's effective query (prefills the inputs). `null`
   *  until the first candidates poll resolves. */
  defaults: SearchQueryView | null;
  disabled: boolean;
  onNewRun: (runId: string) => void;
}) {
  const isIssue = scope.kind === "issue";
  const seriesSearch = useSearchMetadataForSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
  );
  const issueSearch = useSearchMetadataForIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
  );
  const search = isIssue ? issueSearch : seriesSearch;
  const seriesLookup = useLookupMetadataForSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
  );
  const issueLookup = useLookupMetadataForIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
  );
  const lookup = isIssue ? issueLookup : seriesLookup;

  const adjust = useForm<AdjustForm>({
    defaultValues: { name: "", year: "", publisher: "", issue_number: "" },
  });
  // Prefill from the run's effective query once it's known. Only reset
  // while the user hasn't typed, so a poll refresh can't stomp edits.
  const seededFrom = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!defaults) return;
    const key = `${defaults.name}|${defaults.year ?? ""}|${defaults.publisher ?? ""}|${defaults.issue_number ?? ""}`;
    if (seededFrom.current === key || adjust.formState.isDirty) return;
    seededFrom.current = key;
    adjust.reset({
      name: defaults.name,
      year: defaults.year != null ? String(defaults.year) : "",
      publisher: defaults.publisher ?? "",
      issue_number: defaults.issue_number ?? "",
    });
  }, [defaults, adjust]);

  const onAdjust = adjust.handleSubmit(async (values) => {
    const overrides = overridesFromForm(values, defaults, isIssue);
    if (!overrides) {
      adjust.setError("root.serverError", {
        type: "manual",
        message: "Change at least one field to search with a different query.",
      });
      return;
    }
    try {
      const res = await search.mutateAsync(overrides);
      if (res?.run_id) onNewRun(res.run_id);
    } catch (err) {
      // The hook already toasts; bind field errors inline on top.
      applyServerErrors(adjust.setError, err, [
        "name",
        "year",
        "publisher",
        "issue_number",
      ]);
    }
  });

  const lookupForm = useForm<LookupForm>({ defaultValues: { url: "" } });
  const onLookup = lookupForm.handleSubmit(async (values) => {
    const url = values.url.trim();
    if (!url) {
      lookupForm.setError("url", {
        type: "manual",
        message: "Paste a ComicVine, Metron, or GCD URL.",
      });
      return;
    }
    try {
      const res = await lookup.mutateAsync({ url });
      if (res?.run_id) onNewRun(res.run_id);
    } catch (err) {
      applyServerErrors(lookupForm.setError, err, ["url"]);
    }
  });

  const busy = disabled || search.isPending || lookup.isPending;
  const rootError = adjust.formState.errors.root?.serverError?.message;
  const lookupRootError =
    lookupForm.formState.errors.root?.serverError?.message;

  return (
    <div className="border-border/60 mt-2 space-y-3 border-t pt-2">
      <Collapsible>
        <CollapsibleTrigger className="text-muted-foreground hover:text-foreground flex items-center gap-1 text-xs [&[data-state=open]>svg]:rotate-90">
          <ChevronRight className="h-3.5 w-3.5 transition-transform" />
          Adjust query
        </CollapsibleTrigger>
        <CollapsibleContent>
          <Form {...adjust}>
            <form onSubmit={onAdjust} className="mt-2 space-y-2">
              <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <FormField
                  control={adjust.control}
                  name="name"
                  render={({ field }) => (
                    <FormItem className="sm:col-span-2">
                      <FormLabel className="text-xs">Series name</FormLabel>
                      <FormControl>
                        <Input
                          {...field}
                          disabled={busy}
                          placeholder="Search providers for…"
                          maxLength={200}
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={adjust.control}
                  name="year"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs">Start year</FormLabel>
                      <FormControl>
                        <Input
                          {...field}
                          disabled={busy}
                          inputMode="numeric"
                          placeholder="e.g. 2012"
                          maxLength={4}
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={adjust.control}
                  name="publisher"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs">Publisher</FormLabel>
                      <FormControl>
                        <Input {...field} disabled={busy} maxLength={200} />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                {isIssue && (
                  <FormField
                    control={adjust.control}
                    name="issue_number"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-xs">Issue number</FormLabel>
                        <FormControl>
                          <Input
                            {...field}
                            disabled={busy}
                            placeholder="e.g. 12 or Annual 1"
                            maxLength={32}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                )}
              </div>
              <p className="text-muted-foreground text-[11px]">
                Overrides apply to this search only — the series is not edited.
                Setting a year also pins it (no cover-based relaxation).
              </p>
              {rootError && (
                <p className="text-destructive text-xs">{rootError}</p>
              )}
              <div className="flex justify-end">
                <Button
                  type="submit"
                  size="sm"
                  variant="outline"
                  disabled={busy}
                >
                  Search with this query
                </Button>
              </div>
            </form>
          </Form>
        </CollapsibleContent>
      </Collapsible>

      <Form {...lookupForm}>
        <form onSubmit={onLookup} className="space-y-1">
          <FormField
            control={lookupForm.control}
            name="url"
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-muted-foreground flex items-center gap-1 text-xs">
                  <Link2 className="h-3.5 w-3.5" /> Paste provider URL
                </FormLabel>
                <div className="flex gap-2">
                  <FormControl>
                    <Input
                      {...field}
                      disabled={busy}
                      inputMode="url"
                      placeholder="https://comicvine.gamespot.com/…/4050-12345/, metron.cloud/series/1234/, or comics.org/series/1482/"
                    />
                  </FormControl>
                  <Button
                    type="submit"
                    size="sm"
                    variant="outline"
                    disabled={busy}
                  >
                    Lookup
                  </Button>
                </div>
                <FormMessage />
              </FormItem>
            )}
          />
          {lookupRootError && (
            <p className="text-destructive text-xs">{lookupRootError}</p>
          )}
        </form>
      </Form>
    </div>
  );
}
