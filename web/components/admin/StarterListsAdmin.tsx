"use client";

import * as React from "react";
import { Loader2, Plus, RefreshCw, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { StarterRowsSkeleton } from "@/components/skeletons/AdminShapes";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useCatalogEntries, useCatalogSources } from "@/lib/api/queries";
import {
  useAddStarterList,
  useAdminStarterLists,
  useRefreshStarterCoverage,
  useRemoveStarterList,
  useSetStarterListHidden,
  type StarterListView,
} from "@/lib/api/starter-lists";

/**
 * Admin → Starter lists: the reading lists the first-run tour offers new
 * readers. Folio ships a curated set; here an admin hides picks that
 * don't suit this server and adds catalog files of their own. Coverage
 * counts every library (readers see only what they can open).
 */
export function StarterListsAdmin() {
  const q = useAdminStarterLists();
  const setHidden = useSetStarterListHidden();
  const remove = useRemoveStarterList();
  const refresh = useRefreshStarterCoverage();
  const items = q.data?.items ?? [];

  return (
    <div className="space-y-8">
      <section className="space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 className="text-base font-semibold">On the shelf</h2>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={refresh.isPending}
            onClick={() => refresh.mutate()}
          >
            <RefreshCw className="mr-1 size-4" /> Recount coverage
          </Button>
        </div>
        {q.isLoading ? (
          <StarterRowsSkeleton />
        ) : q.isError ? (
          <p className="text-destructive text-sm">
            Failed to load starter lists.
          </p>
        ) : (
          <ul
            role="list"
            className="divide-border/60 divide-y rounded-md border"
          >
            {items.map((item) => (
              <Row
                key={`${item.catalog_source_id}:${item.catalog_path}`}
                item={item}
                busy={setHidden.isPending || remove.isPending}
                onHidden={(hidden) =>
                  setHidden.mutate({
                    catalog_source_id: item.catalog_source_id,
                    catalog_path: item.catalog_path,
                    hidden,
                  })
                }
                onRemove={
                  item.origin === "admin" && item.override_id
                    ? () => remove.mutate(item.override_id!)
                    : undefined
                }
              />
            ))}
          </ul>
        )}
      </section>
      <AddForm />
    </div>
  );
}

function Row({
  item,
  busy,
  onHidden,
  onRemove,
}: {
  item: StarterListView;
  busy: boolean;
  onHidden: (hidden: boolean) => void;
  onRemove?: () => void;
}) {
  const id = `starter-${item.catalog_source_id}-${item.catalog_path}`;
  return (
    <li className="flex items-start gap-3 p-3">
      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <span
            className={item.hidden ? "text-muted-foreground" : "font-medium"}
          >
            {item.title}
          </span>
          <Badge variant="outline" className="font-normal">
            {item.publisher}
          </Badge>
          {item.origin === "admin" ? (
            <Badge variant="secondary" className="font-normal">
              Added here
            </Badge>
          ) : null}
        </div>
        {item.reason ? (
          <p className="text-muted-foreground text-sm">{item.reason}</p>
        ) : null}
        <p className="text-muted-foreground flex items-center gap-1.5 text-xs">
          {item.coverage_status === "pending" ? (
            <>
              <Loader2 className="size-3 animate-spin" /> Counting…
            </>
          ) : item.coverage_status === "error" ? (
            "Couldn't read this catalog file"
          ) : (
            `${item.available} of ${item.total} in your libraries`
          )}
          <span aria-hidden="true">·</span>
          <span className="truncate" title={item.catalog_path}>
            {item.catalog_path}
          </span>
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <Label htmlFor={id} className="text-muted-foreground text-xs">
          Show
        </Label>
        <Switch
          id={id}
          checked={!item.hidden}
          disabled={busy}
          onCheckedChange={(on) => onHidden(!on)}
          aria-label={`Show ${item.title} to new readers`}
        />
        {onRemove ? (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-8"
            disabled={busy}
            onClick={onRemove}
            aria-label={`Remove ${item.title}`}
          >
            <Trash2 className="size-4" />
          </Button>
        ) : null}
      </div>
    </li>
  );
}

function AddForm() {
  const sources = useCatalogSources();
  const add = useAddStarterList();
  const enabled = (sources.data?.items ?? []).filter((s) => s.enabled);
  const [sourceId, setSourceId] = React.useState("");
  const source = sourceId || enabled[0]?.id || "";
  const entries = useCatalogEntries(source);
  const [path, setPath] = React.useState("");
  const [title, setTitle] = React.useState("");
  const [reason, setReason] = React.useState("");

  function submit(e: React.FormEvent) {
    e.preventDefault();
    add.mutate(
      {
        catalog_source_id: source,
        catalog_path: path.trim(),
        title: title.trim() || null,
        reason: reason.trim(),
      },
      {
        onSuccess: () => {
          setPath("");
          setTitle("");
          setReason("");
        },
      },
    );
  }

  return (
    <section className="space-y-3">
      <h2 className="text-base font-semibold">Add a list</h2>
      <form onSubmit={submit} className="max-w-2xl space-y-3">
        {enabled.length > 1 ? (
          <div className="space-y-1.5">
            <Label htmlFor="starter-source">Catalog</Label>
            <select
              id="starter-source"
              value={source}
              onChange={(e) => setSourceId(e.target.value)}
              className="border-input bg-background h-9 w-full rounded-md border px-3 text-sm"
            >
              {enabled.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.display_name}
                </option>
              ))}
            </select>
          </div>
        ) : null}
        <div className="space-y-1.5">
          <Label htmlFor="starter-path">Catalog file</Label>
          <Input
            id="starter-path"
            list="starter-paths"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="Start typing a name, e.g. Saga"
            required
          />
          <datalist id="starter-paths">
            {(entries.data?.items ?? []).map((e) => (
              <option key={e.path} value={e.path} />
            ))}
          </datalist>
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="starter-title">Title</Label>
          <Input
            id="starter-title"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="Defaults to the file name"
            maxLength={120}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="starter-reason">Why start here?</Label>
          <Textarea
            id="starter-reason"
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            rows={2}
            required
            maxLength={300}
            placeholder="One line new readers see under the title"
          />
        </div>
        <Button type="submit" disabled={add.isPending || !source}>
          <Plus className="mr-1 size-4" /> Add to the shelf
        </Button>
      </form>
    </section>
  );
}
