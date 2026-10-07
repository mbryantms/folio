/**
 * Shared presentation helpers for library health-issue rows — used by the
 * per-library health table and the cross-library Findings page so both
 * render every kind the same way.
 *
 * Payloads serialize as adjacently-tagged `{kind, data: {…}}` (see
 * `crates/server/src/library/health.rs::IssueKind`); the synthesized
 * metadata-drift row is flat. `kind` on the wire is a plain string, so
 * unknown kinds fall through to a generic summary.
 */

/** Unwrap a health payload to its field object (nested or flat). */
export function healthPayloadFields(p: unknown): Record<string, unknown> {
  if (!p || typeof p !== "object") return {};
  const obj = p as Record<string, unknown>;
  if (obj.data && typeof obj.data === "object" && !Array.isArray(obj.data)) {
    return obj.data as Record<string, unknown>;
  }
  return obj;
}

type KindCopy = {
  /** Short human title for the kind. */
  label: string;
  /** One-line "what to do about it". */
  hint?: string;
};

const KIND_COPY: Record<string, KindCopy> = {
  FileAtRoot: {
    label: "File at library root",
    hint: "Move it into a series folder.",
  },
  EmptyFolder: {
    label: "Empty folder",
    hint: "Add archives or remove the folder.",
  },
  AmbiguousFolder: {
    label: "Ambiguous folder layout (skipped)",
    hint: "Reshape the folder to Series/archives or Publisher/Series/archives; its archives are not imported until then.",
  },
  UnreadableFile: { label: "Unreadable file" },
  UnreadableArchive: {
    label: "Unreadable archive",
    hint: "Check permissions or replace the file.",
  },
  MissingComicInfo: { label: "Missing ComicInfo.xml" },
  MalformedComicInfo: {
    label: "Malformed ComicInfo.xml",
    hint: "Re-tag the archive.",
  },
  MalformedArchive: {
    label: "Corrupt or unrecognized archive",
    hint: "The file isn't a readable ZIP, TAR, RAR or 7z. Replace it from a good copy.",
  },
  FolderNameMismatch: {
    label: "Folder name ≠ ComicInfo series",
    hint: "ComicInfo wins for the series name. Rename the folder, or re-tag the archives if the folder is right.",
  },
  MixedSeriesInFolder: {
    label: "Mixed series in one folder",
    hint: "Archives with a different <Series> are likely misfiled — move them to their own series folder.",
  },
  DuplicateContent: {
    label: "Duplicate content",
    hint: "Decide which copy to keep.",
  },
  OrphanedSeriesJson: {
    label: "Orphaned series.json",
    hint: "The folder has a series.json but no archives — restore the archives or delete the folder.",
  },
  UnsupportedArchiveFormat: {
    label: "Unsupported archive format",
    hint: "Turn on CBR / CB7 → CBZ conversion for the library, or convert the file to CBZ yourself.",
  },
  RecoveredArchive: { label: "Recovered archive" },
  SkippedArchiveEntries: { label: "Skipped archive entries" },
  UnreadablePage: { label: "Unreadable page" },
  NoPages: { label: "Archive has no pages" },
  DuplicateExternalId: { label: "Duplicate provider ID" },
  MetadataDriftFromXml: { label: "Edits not written to XML" },
};

/** Human title for a health kind (falls back to the raw kind). */
export function healthKindLabel(kind: string): string {
  return KIND_COPY[kind]?.label ?? kind;
}

/** Optional fix hint for a health kind. */
export function healthKindHint(kind: string): string | undefined {
  return KIND_COPY[kind]?.hint;
}

function str(v: unknown): string | undefined {
  return typeof v === "string" && v.length > 0 ? v : undefined;
}

function num(v: unknown): number | undefined {
  return typeof v === "number" ? v : undefined;
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** One-line summary for a health row. */
export function healthPayloadSummary(kind: string, p: unknown): string {
  const obj = healthPayloadFields(p);
  if (Object.keys(obj).length === 0) return "";
  const path = str(obj.path) ?? str(obj.file_path) ?? "";

  switch (kind) {
    case "MetadataDriftFromXml": {
      const issues = num(obj.drifted_issue_count) ?? "?";
      const series = num(obj.drifted_series_count) ?? "?";
      return `${issues} issue${issues === 1 ? "" : "s"} across ${series} series ${
        issues === 1 ? "has" : "have"
      } user edits not yet written to XML`;
    }
    case "RecoveredArchive": {
      const technique = str(obj.technique) ?? "unknown";
      return path
        ? `${path} — recovered (${technique})`
        : `recovered (${technique})`;
    }
    case "SkippedArchiveEntries": {
      const dropped = num(obj.dropped) ?? "?";
      const total = num(obj.total) ?? "?";
      const reason = str(obj.reason) ?? "soft defense";
      const suffix = `${dropped} of ${total} entries dropped (${reason})`;
      return path ? `${path} — ${suffix}` : suffix;
    }
    case "FolderNameMismatch": {
      const folder = str(obj.folder) ?? "";
      const series = str(obj.comic_info_series) ?? "?";
      const files = num(obj.files);
      const count = files !== undefined ? ` (${plural(files, "file")})` : "";
      return `${folder} — ComicInfo says “${series}”${count}`;
    }
    case "MixedSeriesInFolder": {
      const folder = str(obj.folder) ?? "";
      const distinct = num(obj.distinct_values) ?? mixedSeriesValues(p).length;
      return `${folder} — ${distinct} different ComicInfo series`;
    }
    case "AmbiguousFolder": {
      const reason = str(obj.reason);
      const count = num(obj.skipped_archive_count);
      const parts = [path];
      if (reason) parts.push(reason);
      if (count !== undefined)
        parts.push(`${plural(count, "archive")} skipped`);
      return parts.filter(Boolean).join(" — ");
    }
    case "UnsupportedArchiveFormat": {
      // `ext` is the *container* the scanner found (cbr / cb7). When it
      // differs from the file's own extension the extension lied — say so,
      // because "unsupported format" on a `.cbz` otherwise reads as a bug.
      const ext = str(obj.ext);
      if (!ext) break;
      const named = path.toLowerCase().match(/\.([a-z0-9]+)$/)?.[1];
      const container = ext.toUpperCase();
      const suffix =
        named && named !== ext.toLowerCase()
          ? `${container} archive mislabeled as .${named}`
          : `${container} archive`;
      return path ? `${path} — ${suffix}` : suffix;
    }
    case "OrphanedSeriesJson":
      return str(obj.folder) ?? "";
    case "DuplicateContent": {
      const a = str(obj.path_a);
      const b = str(obj.path_b);
      if (a && b) return `${a} ≡ ${b}`;
      break;
    }
  }

  const parts: string[] = [];
  for (const k of [
    "path",
    "file_path",
    "folder",
    "reason",
    "error",
    "details",
  ]) {
    const v = str(obj[k]);
    if (v) parts.push(v);
  }
  if (parts.length > 0) return parts.join(" — ");
  return JSON.stringify(obj);
}

export type MixedSeriesValue = {
  series: string;
  files: number;
  example?: string;
};

/** The distinct `<Series>` values a `MixedSeriesInFolder` row lists. */
export function mixedSeriesValues(p: unknown): MixedSeriesValue[] {
  const values = healthPayloadFields(p).series_values;
  if (!Array.isArray(values)) return [];
  const out: MixedSeriesValue[] = [];
  for (const v of values) {
    // Tolerate a bare-string shape too.
    if (typeof v === "string") {
      out.push({ series: v, files: 0 });
    } else if (v && typeof v === "object") {
      const o = v as Record<string, unknown>;
      const series = str(o.series);
      if (!series) continue;
      out.push({ series, files: num(o.files) ?? 0, example: str(o.example) });
    }
  }
  return out;
}

/**
 * Expandable detail lines for kinds that carry a list: the skipped-subtree
 * preview on `AmbiguousFolder`, the per-value breakdown on
 * `MixedSeriesInFolder`. `more` counts entries the bounded payload left out.
 */
export function healthPayloadDetails(
  kind: string,
  p: unknown,
): { title: string; lines: string[]; more: number } | null {
  const obj = healthPayloadFields(p);
  if (kind === "AmbiguousFolder") {
    const list = Array.isArray(obj.skipped_archives)
      ? obj.skipped_archives.filter((s): s is string => typeof s === "string")
      : [];
    if (list.length === 0) return null;
    const total = num(obj.skipped_archive_count) ?? list.length;
    return {
      title: "Skipped archives",
      lines: list,
      more: Math.max(0, total - list.length),
    };
  }
  if (kind === "MixedSeriesInFolder") {
    const values = mixedSeriesValues(p);
    if (values.length === 0) return null;
    const distinct = num(obj.distinct_values) ?? values.length;
    return {
      title: "Series values",
      lines: values.map((v) => {
        const count = v.files > 0 ? ` — ${plural(v.files, "file")}` : "";
        const eg = v.example ? ` (e.g. ${v.example})` : "";
        return `${v.series}${count}${eg}`;
      }),
      more: Math.max(0, distinct - values.length),
    };
  }
  return null;
}

/** Entity refs a health payload may carry (audit UX-3). `issue_id` rides
 *  the `/issues/{id}` permalink redirect; series ids resolve directly —
 *  `/series/{slug}` accepts a UUID. */
export function healthPayloadRefs(p: unknown): {
  seriesIds: string[];
  issueId?: string;
} {
  const obj = healthPayloadFields(p);
  const seriesIds: string[] = [];
  if (typeof obj.series_id === "string") seriesIds.push(obj.series_id);
  if (Array.isArray(obj.affected_series_ids)) {
    for (const v of obj.affected_series_ids) {
      if (typeof v === "string") seriesIds.push(v);
    }
  }
  return {
    seriesIds,
    issueId: typeof obj.issue_id === "string" ? obj.issue_id : undefined,
  };
}
