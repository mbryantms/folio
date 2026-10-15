/**
 * Per-series reader overrides that follow the account (post-audit T4.4).
 *
 * The reader keeps four choices per series — fit mode, view mode, reading
 * direction, cover solo — in localStorage (`prefs-storage`). This module
 * keeps that local cache in step with the server row at
 * `/me/series/{id}/reader-prefs`:
 *
 *   - when the reader opens, {@link mirrorServerPrefs} copies the server's
 *     slices over the local ones (server > local), and
 *     {@link pendingAdoption} reports local overrides the server has
 *     never seen so they can be pushed once — a legacy per-browser
 *     override becomes an account override the first time it is read
 *     online;
 *   - every per-series write in the store calls {@link pushSeriesPref},
 *     which coalesces the slices changed within a short window into one
 *     PUT (the `f` / `v` cycle keys can fire several times a second).
 *
 * localStorage stays the offline cache: with no network the store still
 * reads and writes it, and the next online open pushes what changed.
 * The transport is injectable so the store's unit tests never touch the
 * network.
 */

import type {
  SeriesReaderPrefsPatch,
  SeriesReaderPrefsView,
} from "@/lib/api/types";
import { readerPrefGet, readerPrefSet } from "@/lib/reader/prefs-storage";

export type SeriesPrefSlice =
  "fitMode" | "viewMode" | "direction" | "coverSolo";

export const SERIES_PREF_SLICES: readonly SeriesPrefSlice[] = [
  "fitMode",
  "viewMode",
  "direction",
  "coverSolo",
];

const FIELD_OF: Record<SeriesPrefSlice, keyof SeriesReaderPrefsView> = {
  fitMode: "fit_mode",
  viewMode: "view_mode",
  direction: "reading_direction",
  coverSolo: "cover_solo",
};

/** The localStorage string for a server slice value. */
function toLocal(value: string | boolean): string {
  return typeof value === "boolean" ? (value ? "true" : "false") : value;
}

/** The server value for a localStorage string. `coverSolo` is a boolean
 *  on the wire; everything else is its token. */
function toServer(slice: SeriesPrefSlice, value: string): string | boolean {
  return slice === "coverSolo" ? value === "true" : value;
}

/** Copy every slice the server has over the local cache. Slices the
 *  server lacks are left alone (they may be offline edits or a legacy
 *  override awaiting adoption — see {@link pendingAdoption}). */
export function mirrorServerPrefs(
  seriesId: string,
  view: SeriesReaderPrefsView,
): void {
  for (const slice of SERIES_PREF_SLICES) {
    const v = view[FIELD_OF[slice]];
    if (v === undefined || v === null) continue;
    readerPrefSet(slice, seriesId, toLocal(v));
  }
}

/** Local overrides the server row does not carry, as the PUT body that
 *  would adopt them; `null` when the server already has everything local
 *  has. Called once per reader open, after {@link mirrorServerPrefs}. */
export function pendingAdoption(
  seriesId: string,
  view: SeriesReaderPrefsView,
): SeriesReaderPrefsPatch | null {
  const patch: Record<string, string | boolean> = {};
  for (const slice of SERIES_PREF_SLICES) {
    const server = view[FIELD_OF[slice]];
    if (server !== undefined && server !== null) continue;
    const local = readerPrefGet(slice, seriesId);
    if (local === null) continue;
    patch[FIELD_OF[slice]] = toServer(slice, local);
  }
  return Object.keys(patch).length === 0
    ? null
    : (patch as SeriesReaderPrefsPatch);
}

/** The PUT body for one slice write. */
export function patchForSlice(
  slice: SeriesPrefSlice,
  value: string,
): SeriesReaderPrefsPatch {
  return {
    [FIELD_OF[slice]]: toServer(slice, value),
  } as SeriesReaderPrefsPatch;
}

export type SeriesPrefTransport = (
  seriesId: string,
  patch: SeriesReaderPrefsPatch,
) => Promise<unknown>;

let transport: SeriesPrefTransport | null = null;

/** Install the network sender (the Reader does this on mount with the
 *  API client; tests install a spy). `null` turns pushes into no-ops. */
export function setSeriesPrefTransport(fn: SeriesPrefTransport | null): void {
  transport = fn;
}

/** How long changed slices are batched before one PUT goes out. */
export const PUSH_COALESCE_MS = 400;

const pending = new Map<
  string,
  { patch: SeriesReaderPrefsPatch; timer: ReturnType<typeof setTimeout> }
>();

/** Send `patch` for `seriesId` after the coalescing window, merged with
 *  anything else queued for the series in the meantime. Failures are
 *  swallowed: the local cache already holds the change and the next
 *  online open adopts it. */
export function pushSeriesPrefs(
  seriesId: string,
  patch: SeriesReaderPrefsPatch,
): void {
  if (!transport) return;
  const existing = pending.get(seriesId);
  if (existing) {
    clearTimeout(existing.timer);
    existing.patch = { ...existing.patch, ...patch };
    existing.timer = setTimeout(() => flush(seriesId), PUSH_COALESCE_MS);
    return;
  }
  pending.set(seriesId, {
    patch,
    timer: setTimeout(() => flush(seriesId), PUSH_COALESCE_MS),
  });
}

/** One slice write from the store. */
export function pushSeriesPref(
  seriesId: string,
  slice: SeriesPrefSlice,
  value: string,
): void {
  pushSeriesPrefs(seriesId, patchForSlice(slice, value));
}

function flush(seriesId: string): void {
  const entry = pending.get(seriesId);
  pending.delete(seriesId);
  if (!entry || !transport) return;
  void transport(seriesId, entry.patch).catch(() => undefined);
}

/** Send everything queued now (tests, and the Reader's unmount). */
export function flushSeriesPrefs(): void {
  for (const [seriesId, entry] of pending) {
    clearTimeout(entry.timer);
    pending.delete(seriesId);
    if (transport) void transport(seriesId, entry.patch).catch(() => undefined);
  }
}
