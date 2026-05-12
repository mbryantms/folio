"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import {
  ChevronRight,
  FileText,
  Folder,
  Globe,
  Library,
  Loader2,
  Search,
  Upload,
} from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { useCatalogEntries, useCatalogSources } from "@/lib/api/queries";
import {
  useCreateCblList,
  useCreateSavedView,
  uploadCblFile,
} from "@/lib/api/mutations";
import type { CblListView, CatalogEntryView } from "@/lib/api/types";

const REFRESH_OPTIONS: { value: string; label: string }[] = [
  { value: "manual", label: "Manual only" },
  { value: "@daily", label: "Daily" },
  { value: "@weekly", label: "Weekly" },
  { value: "@monthly", label: "Monthly" },
];

type Step = "source" | "save";

export function CblImportDialog({
  open,
  onOpenChange,
  /** Optional callback after a saved-view row lands. The dialog closes
   *  itself either way; pass a navigator to push the user onto the new
   *  detail page. */
  onSaved,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved?: (savedViewId: string) => void;
}) {
  const router = useRouter();
  const [step, setStep] = React.useState<Step>("source");
  const [importedList, setImportedList] = React.useState<CblListView | null>(
    null,
  );

  // Reset state inside the open-change handler instead of a useEffect: the
  // close transition is the user-visible boundary that should reset, and
  // doing it here keeps state writes in event handlers (avoids the
  // react-hooks/set-state-in-effect lint).
  function handleOpenChange(next: boolean) {
    if (!next) {
      setStep("source");
      setImportedList(null);
    }
    onOpenChange(next);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="flex max-h-[85vh] max-w-3xl flex-col overflow-hidden">
        <DialogHeader>
          <DialogTitle>
            {step === "source" ? "Import CBL list" : "Save as view"}
          </DialogTitle>
          <DialogDescription>
            {step === "source"
              ? "Pull a reading list from the community catalog, fetch it from a URL, or upload a .cbl file."
              : "Name this view and choose how often the source should refresh."}
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-y-auto">
          {step === "source" ? (
            <SourceStep
              onImported={(list) => {
                setImportedList(list);
                setStep("save");
              }}
            />
          ) : importedList ? (
            <SaveStep
              list={importedList}
              onCancel={() => setStep("source")}
              onSaved={(savedViewId) => {
                onOpenChange(false);
                if (onSaved) onSaved(savedViewId);
                else router.push(`/views/${savedViewId}`);
              }}
            />
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function SourceStep({
  onImported,
}: {
  onImported: (list: CblListView) => void;
}) {
  return (
    <Tabs defaultValue="catalog">
      <TabsList>
        <TabsTrigger value="catalog">
          <Library className="mr-1 h-4 w-4" /> Browse catalog
        </TabsTrigger>
        <TabsTrigger value="url">
          <Globe className="mr-1 h-4 w-4" /> From URL
        </TabsTrigger>
        <TabsTrigger value="upload">
          <Upload className="mr-1 h-4 w-4" /> Upload file
        </TabsTrigger>
      </TabsList>
      <TabsContent value="catalog">
        <CatalogTab onImported={onImported} />
      </TabsContent>
      <TabsContent value="url">
        <UrlTab onImported={onImported} />
      </TabsContent>
      <TabsContent value="upload">
        <UploadTab onImported={onImported} />
      </TabsContent>
    </Tabs>
  );
}

function CatalogTab({
  onImported,
}: {
  onImported: (list: CblListView) => void;
}) {
  const sources = useCatalogSources();
  const [chosen, setChosen] = React.useState<string | null>(null);
  // Derived default — falls back to the first available source when the
  // user hasn't picked yet. Avoids a setState-in-effect roundtrip.
  const sourceId = chosen ?? sources.data?.items[0]?.id ?? "";
  const setSourceId = (next: string) => setChosen(next);

  if (sources.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading sources…
      </div>
    );
  }
  if (!sources.data || sources.data.items.length === 0) {
    return (
      <div className="text-muted-foreground rounded-md border border-dashed p-6 text-sm">
        No catalog sources are configured. Ask an admin to add one in Settings →
        Library → CBL catalog source.
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <Label htmlFor="catalog-source">Source</Label>
        <Select value={sourceId} onValueChange={setSourceId}>
          <SelectTrigger id="catalog-source">
            <SelectValue placeholder="Pick a source…" />
          </SelectTrigger>
          <SelectContent>
            {sources.data.items.map((s) => (
              <SelectItem key={s.id} value={s.id}>
                {s.display_name} · {s.github_owner}/{s.github_repo}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {sourceId ? (
        // Key on sourceId so switching sources resets breadcrumb / search
        // state without us reaching into refs from render.
        <CatalogList
          key={sourceId}
          sourceId={sourceId}
          onImported={onImported}
        />
      ) : null}
    </div>
  );
}

/** One folder node in the catalog tree. `files` is direct file children
 *  in this folder; `folders` is the nested-folder map keyed by the
 *  segment name. The root node has `name === ""`. */
type FolderNode = {
  name: string;
  fullPath: string;
  files: CatalogEntryView[];
  folders: Map<string, FolderNode>;
  /** Total file count in this subtree — surfaced as the row counter. */
  fileCount: number;
};

function buildTree(entries: CatalogEntryView[]): FolderNode {
  const root: FolderNode = {
    name: "",
    fullPath: "",
    files: [],
    folders: new Map(),
    fileCount: 0,
  };
  for (const entry of entries) {
    const segments = entry.path.split("/");
    const folders = segments.slice(0, -1);
    let cur = root;
    let curPath = "";
    for (const seg of folders) {
      curPath = curPath ? `${curPath}/${seg}` : seg;
      let next = cur.folders.get(seg);
      if (!next) {
        next = {
          name: seg,
          fullPath: curPath,
          files: [],
          folders: new Map(),
          fileCount: 0,
        };
        cur.folders.set(seg, next);
      }
      cur = next;
    }
    cur.files.push(entry);
  }
  // Recursively roll up file counts and sort.
  function finalize(node: FolderNode): number {
    let total = node.files.length;
    for (const child of node.folders.values()) {
      total += finalize(child);
    }
    node.fileCount = total;
    node.files.sort((a, b) => a.name.localeCompare(b.name));
    node.folders = new Map(
      Array.from(node.folders.entries()).sort(([a], [b]) => a.localeCompare(b)),
    );
    return total;
  }
  finalize(root);
  return root;
}

function findNode(root: FolderNode, segments: string[]): FolderNode | null {
  let cur: FolderNode = root;
  for (const seg of segments) {
    const next = cur.folders.get(seg);
    if (!next) return null;
    cur = next;
  }
  return cur;
}

function CatalogList({
  sourceId,
  onImported,
}: {
  sourceId: string;
  onImported: (list: CblListView) => void;
}) {
  const entries = useCatalogEntries(sourceId);
  const [search, setSearch] = React.useState("");
  /** Folder navigation breadcrumb. Empty array means the root. */
  const [pathSegments, setPathSegments] = React.useState<string[]>([]);
  const create = useCreateCblList();
  const [pendingPath, setPendingPath] = React.useState<string | null>(null);

  const tree = React.useMemo(
    () => (entries.data ? buildTree(entries.data.items) : null),
    [entries.data],
  );

  const currentNode = React.useMemo(() => {
    if (!tree) return null;
    return findNode(tree, pathSegments) ?? tree;
  }, [tree, pathSegments]);

  /** Flat search results across the entire source — bypasses the folder
   *  hierarchy when the user is hunting by name. */
  const searchResults = React.useMemo(() => {
    if (!entries.data) return [] as CatalogEntryView[];
    const lower = search.trim().toLowerCase();
    if (!lower) return [];
    return entries.data.items
      .filter(
        (e) =>
          e.name.toLowerCase().includes(lower) ||
          e.path.toLowerCase().includes(lower),
      )
      .sort((a, b) => a.name.localeCompare(b.name))
      .slice(0, 200);
  }, [entries.data, search]);

  if (entries.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading catalog…
      </div>
    );
  }
  if (entries.isError) {
    return (
      <div className="text-destructive rounded-md border p-4 text-sm">
        Failed to load catalog: {String(entries.error)}
      </div>
    );
  }
  if (!entries.data || !tree || !currentNode) return null;

  async function importEntry(entry: CatalogEntryView) {
    setPendingPath(entry.path);
    try {
      const list = await create.mutateAsync({
        kind: "catalog",
        catalog_source_id: sourceId,
        catalog_path: entry.path,
      });
      if (list) onImported(list);
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : "Failed to import from catalog",
      );
    } finally {
      setPendingPath(null);
    }
  }

  const searchActive = search.trim() !== "";

  return (
    <div className="flex flex-col gap-3">
      <div className="border-input flex items-center gap-2 rounded-md border px-3">
        <Search className="text-muted-foreground h-4 w-4" />
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search by name or path…"
          className="placeholder:text-muted-foreground h-9 flex-1 bg-transparent text-sm outline-none"
        />
      </div>
      {!searchActive ? (
        <Breadcrumbs
          segments={pathSegments}
          onNavigate={(next) => setPathSegments(next)}
        />
      ) : null}
      <div className="border-border/60 max-h-[50vh] overflow-auto rounded-md border">
        {searchActive ? (
          <SearchResults
            results={searchResults}
            onImport={importEntry}
            isImporting={(path) => create.isPending && pendingPath === path}
          />
        ) : (
          <FolderView
            node={currentNode}
            onOpenFolder={(folder) =>
              setPathSegments(folder.fullPath.split("/"))
            }
            onImport={importEntry}
            isImporting={(path) => create.isPending && pendingPath === path}
          />
        )}
      </div>
    </div>
  );
}

function Breadcrumbs({
  segments,
  onNavigate,
}: {
  segments: string[];
  onNavigate: (next: string[]) => void;
}) {
  return (
    <nav
      aria-label="Catalog folder path"
      className="text-muted-foreground flex flex-wrap items-center gap-1 text-xs"
    >
      <button
        type="button"
        onClick={() => onNavigate([])}
        className={`hover:text-foreground transition-colors ${
          segments.length === 0 ? "text-foreground font-medium" : ""
        }`}
      >
        All publishers
      </button>
      {segments.map((seg, i) => (
        <React.Fragment key={i}>
          <ChevronRight className="h-3 w-3 shrink-0" aria-hidden />
          <button
            type="button"
            onClick={() => onNavigate(segments.slice(0, i + 1))}
            className={`hover:text-foreground transition-colors ${
              i === segments.length - 1 ? "text-foreground font-medium" : ""
            }`}
          >
            {seg}
          </button>
        </React.Fragment>
      ))}
    </nav>
  );
}

function FolderView({
  node,
  onOpenFolder,
  onImport,
  isImporting,
}: {
  node: FolderNode;
  onOpenFolder: (folder: FolderNode) => void;
  onImport: (entry: CatalogEntryView) => void;
  isImporting: (path: string) => boolean;
}) {
  const folders = Array.from(node.folders.values());
  if (folders.length === 0 && node.files.length === 0) {
    return (
      <div className="text-muted-foreground py-6 text-center text-sm">
        Empty folder.
      </div>
    );
  }
  return (
    <ul className="divide-border/60 divide-y">
      {folders.map((folder) => (
        <li key={folder.fullPath}>
          <button
            type="button"
            onClick={() => onOpenFolder(folder)}
            className="hover:bg-accent/40 flex w-full items-center justify-between gap-3 px-3 py-2 text-left transition-colors"
          >
            <span className="flex min-w-0 items-center gap-2">
              <Folder className="text-muted-foreground h-4 w-4 shrink-0" />
              <span className="truncate text-sm font-medium">
                {folder.name}
              </span>
            </span>
            <span className="text-muted-foreground flex shrink-0 items-center gap-2 text-xs">
              <span>
                {folder.fileCount} {folder.fileCount === 1 ? "list" : "lists"}
              </span>
              <ChevronRight className="h-3 w-3" aria-hidden />
            </span>
          </button>
        </li>
      ))}
      {node.files.map((entry) => (
        <li key={entry.path}>
          <FileRow
            entry={entry}
            onImport={onImport}
            isImporting={isImporting(entry.path)}
          />
        </li>
      ))}
    </ul>
  );
}

function SearchResults({
  results,
  onImport,
  isImporting,
}: {
  results: CatalogEntryView[];
  onImport: (entry: CatalogEntryView) => void;
  isImporting: (path: string) => boolean;
}) {
  if (results.length === 0) {
    return (
      <div className="text-muted-foreground py-6 text-center text-sm">
        No matching lists.
      </div>
    );
  }
  return (
    <ul className="divide-border/60 divide-y">
      {results.map((entry) => (
        <li key={entry.path}>
          <FileRow
            entry={entry}
            onImport={onImport}
            isImporting={isImporting(entry.path)}
            showPath
          />
        </li>
      ))}
    </ul>
  );
}

function FileRow({
  entry,
  onImport,
  isImporting,
  showPath,
}: {
  entry: CatalogEntryView;
  onImport: (entry: CatalogEntryView) => void;
  isImporting: boolean;
  showPath?: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-3 px-3 py-2">
      <div className="flex min-w-0 items-center gap-2">
        <FileText className="text-muted-foreground h-4 w-4 shrink-0" />
        <div className="min-w-0">
          <div className="truncate text-sm font-medium" title={entry.name}>
            {entry.name}
          </div>
          {showPath ? (
            <div
              className="text-muted-foreground truncate text-xs"
              title={entry.path}
            >
              {entry.path}
            </div>
          ) : null}
        </div>
      </div>
      <Button
        type="button"
        size="sm"
        onClick={() => onImport(entry)}
        disabled={isImporting}
      >
        {isImporting ? (
          <>
            <Loader2 className="mr-1 h-3 w-3 animate-spin" />
            Importing…
          </>
        ) : (
          "Import"
        )}
      </Button>
    </div>
  );
}

function UrlTab({ onImported }: { onImported: (list: CblListView) => void }) {
  const [url, setUrl] = React.useState("");
  const [name, setName] = React.useState("");
  const create = useCreateCblList();

  async function submit(ev: React.FormEvent) {
    ev.preventDefault();
    if (!url.trim()) return;
    try {
      const list = await create.mutateAsync({
        kind: "url",
        url: url.trim(),
        name: name.trim() || undefined,
      });
      if (list) onImported(list);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Failed to import from URL");
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <Label htmlFor="cbl-url">CBL URL</Label>
        <Input
          id="cbl-url"
          type="url"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://example.com/reading-order.cbl"
          required
        />
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor="cbl-name-override">Name (optional)</Label>
        <Input
          id="cbl-name-override"
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Falls back to the file's `<Name>` element"
        />
      </div>
      <div>
        <Button type="submit" disabled={create.isPending || url.trim() === ""}>
          {create.isPending ? (
            <>
              <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Importing…
            </>
          ) : (
            "Import"
          )}
        </Button>
      </div>
    </form>
  );
}

function UploadTab({
  onImported,
}: {
  onImported: (list: CblListView) => void;
}) {
  const [file, setFile] = React.useState<File | null>(null);
  const [name, setName] = React.useState("");
  const [submitting, setSubmitting] = React.useState(false);
  const dropRef = React.useRef<HTMLDivElement>(null);
  const [dragOver, setDragOver] = React.useState(false);

  async function submit() {
    if (!file) return;
    setSubmitting(true);
    try {
      const list = await uploadCblFile(file, {
        name: name.trim() || undefined,
      });
      onImported(list);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Upload failed");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <div
        ref={dropRef}
        onDragOver={(e) => {
          e.preventDefault();
          setDragOver(true);
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragOver(false);
          const f = e.dataTransfer.files?.[0];
          if (f) setFile(f);
        }}
        className={`flex h-32 flex-col items-center justify-center gap-2 rounded-md border border-dashed p-4 text-sm transition-colors ${
          dragOver ? "bg-accent" : "bg-muted/30"
        }`}
      >
        <Upload className="text-muted-foreground h-6 w-6" />
        {file ? (
          <div className="text-center">
            <div className="font-medium">{file.name}</div>
            <div className="text-muted-foreground text-xs">
              {(file.size / 1024).toFixed(1)} KiB
            </div>
          </div>
        ) : (
          <div className="text-muted-foreground text-center">
            Drop a `.cbl` file here, or
            <label className="text-foreground ml-1 cursor-pointer underline">
              browse
              <input
                type="file"
                accept=".cbl,application/xml,text/xml"
                hidden
                onChange={(e) => setFile(e.target.files?.[0] ?? null)}
              />
            </label>
          </div>
        )}
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor="upload-name">Name (optional)</Label>
        <Input
          id="upload-name"
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Falls back to the file's `<Name>` element"
        />
      </div>
      <div>
        <Button type="button" onClick={submit} disabled={submitting || !file}>
          {submitting ? (
            <>
              <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Uploading…
            </>
          ) : (
            "Upload"
          )}
        </Button>
      </div>
      <p className="text-muted-foreground text-xs">
        Limits: 4 MiB / 5 000 entries.
      </p>
    </div>
  );
}

function SaveStep({
  list,
  onCancel,
  onSaved,
}: {
  list: CblListView;
  onCancel: () => void;
  onSaved: (savedViewId: string) => void;
}) {
  const [name, setName] = React.useState(list.parsed_name);
  const [description, setDescription] = React.useState(list.description ?? "");
  const [tagsRaw, setTagsRaw] = React.useState("");
  const [yearStart, setYearStart] = React.useState("");
  const [yearEnd, setYearEnd] = React.useState("");
  const defaultSchedule =
    list.refresh_schedule ??
    (list.source_kind === "catalog" ? "@weekly" : "manual");
  const [schedule, setSchedule] = React.useState(defaultSchedule);
  const create = useCreateSavedView();

  async function save() {
    const tags = tagsRaw
      .split(",")
      .map((t) => t.trim())
      .filter(Boolean);
    try {
      const view = await create.mutateAsync({
        kind: "cbl",
        name: name.trim(),
        description: description.trim() || null,
        custom_tags: tags.length ? tags : null,
        custom_year_start: yearStart ? parseInt(yearStart, 10) : null,
        custom_year_end: yearEnd ? parseInt(yearEnd, 10) : null,
        cbl_list_id: list.id,
      });
      // Note: refresh_schedule lives on cbl_lists, not saved_views; the
      // import endpoint defaults it per source kind, and `schedule` here
      // is rendered for visibility — wiring the PATCH happens on the
      // detail page Settings tab.
      void schedule;
      if (!view) throw new Error("Saved-view create returned empty");
      toast.success("View created");
      onSaved(view.id);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Failed to create view");
    }
  }

  const stats = list.stats;

  return (
    <div className="flex flex-col gap-4">
      <div className="bg-muted/50 grid grid-cols-2 gap-4 rounded-md p-3 text-sm sm:grid-cols-4">
        <Stat label="Total" value={stats.total} />
        <Stat label="Matched" value={stats.matched} tone="ok" />
        <Stat label="Ambiguous" value={stats.ambiguous} tone="warn" />
        <Stat label="Missing" value={stats.missing} tone="bad" />
      </div>
      {list.parsed_matchers_present ? (
        <div className="border-warning bg-warning/10 rounded-md border p-3 text-sm">
          This file contains rule-based <code>{`<Matchers>`}</code>. Folio only
          imports the static <code>{`<Books>`}</code> portion.
        </div>
      ) : null}
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1">
          <Label htmlFor="cbl-save-name">Name</Label>
          <Input
            id="cbl-save-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="cbl-save-schedule">Refresh schedule</Label>
          <Select value={schedule} onValueChange={(v) => setSchedule(v)}>
            <SelectTrigger id="cbl-save-schedule">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {REFRESH_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>
                  {o.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor="cbl-save-desc">Description</Label>
        <Textarea
          id="cbl-save-desc"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={2}
        />
      </div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <div className="flex flex-col gap-1">
          <Label htmlFor="cbl-save-tags">Tags (comma-separated)</Label>
          <Input
            id="cbl-save-tags"
            value={tagsRaw}
            onChange={(e) => setTagsRaw(e.target.value)}
            placeholder="event, big-two"
          />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="cbl-save-year-start">Year from</Label>
          <Input
            id="cbl-save-year-start"
            type="number"
            value={yearStart}
            onChange={(e) => setYearStart(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="cbl-save-year-end">Year to</Label>
          <Input
            id="cbl-save-year-end"
            type="number"
            value={yearEnd}
            onChange={(e) => setYearEnd(e.target.value)}
          />
        </div>
      </div>
      <div className="flex items-center justify-between">
        <Button type="button" variant="ghost" onClick={onCancel}>
          Back
        </Button>
        <Button
          type="button"
          onClick={save}
          disabled={create.isPending || name.trim() === ""}
        >
          {create.isPending ? (
            <>
              <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Saving…
            </>
          ) : (
            "Save view"
          )}
        </Button>
      </div>
    </div>
  );
}

function Stat({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone?: "ok" | "warn" | "bad";
}) {
  const toneClass =
    tone === "ok"
      ? "text-emerald-600 dark:text-emerald-400"
      : tone === "warn"
        ? "text-amber-600 dark:text-amber-400"
        : tone === "bad"
          ? "text-rose-600 dark:text-rose-400"
          : "";
  return (
    <div>
      <div className="text-muted-foreground text-xs tracking-wider uppercase">
        {label}
      </div>
      <div className={`text-2xl font-semibold ${toneClass}`}>{value}</div>
    </div>
  );
}
