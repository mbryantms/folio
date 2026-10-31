/**
 * Account-wide reader display preferences that follow the account (audit
 * 2026-11 §5 item 24, remediation F7).
 *
 * Brightness, sepia, "keep zoom between pages" and the colour filters
 * (grayscale, invert, warmth, contrast) are global to the viewer — they
 * are about the screen and the room, not one series — and lived only in
 * this browser's localStorage (`prefs-storage`, `_default` scope). They
 * now mirror the account's `/me/preferences` row the way the per-series
 * slices mirror `/me/series/{id}/reader-prefs` (`series-prefs-sync`):
 *
 *   - when the reader opens, {@link mirrorAccountPrefs} copies every slice
 *     the account has set over the local one (account > local), so a
 *     setting made on another device is what this one shows;
 *   - slices the account has **never** set (`null`) leave the local value
 *     in charge, and {@link noteAccountAdoption} remembers them. They are
 *     adopted — written to the account — on the **first change** the user
 *     makes in this session, folded into that PATCH, not on read: a reader
 *     who only ever opens issues never triggers a write, and nobody loses
 *     a setting their browser already had;
 *   - every global write in the store calls {@link pushAccountPref}, which
 *     coalesces the slices changed within a short window into one PATCH
 *     (a slider drag fires many times a second).
 *
 * The layering stays what it was for the series slices: the account value
 * is the default, localStorage is the offline cache. There is no
 * per-series override for these (there never was). The transport is
 * injectable so the store's unit tests never touch the network; the
 * Reader installs it with the account mutation.
 */

import type { MeView, PreferencesReq } from "@/lib/api/types";
import {
  clampBrightness,
  clampContrast,
  clampGrayscale,
  clampSepia,
  clampWarmth,
} from "@/lib/reader/filters";
import {
  DEFAULT_SCOPE,
  readerPrefGet,
  readerPrefSet,
} from "@/lib/reader/prefs-storage";

export type AccountPrefSlice =
  | "brightness"
  | "sepia"
  | "grayscale"
  | "invert"
  | "warmth"
  | "contrast"
  | "zoomPersist";

export const ACCOUNT_PREF_SLICES: readonly AccountPrefSlice[] = [
  "brightness",
  "sepia",
  "grayscale",
  "invert",
  "warmth",
  "contrast",
  "zoomPersist",
];

/** The account's reader display slices, as `/auth/me` carries them. */
export type AccountReaderPrefs = Pick<
  MeView,
  | "reader_brightness"
  | "reader_sepia"
  | "reader_grayscale"
  | "reader_invert"
  | "reader_warmth"
  | "reader_contrast"
  | "reader_zoom_persist"
>;

type AccountField = keyof AccountReaderPrefs;

const FIELD_OF: Record<AccountPrefSlice, AccountField> = {
  brightness: "reader_brightness",
  sepia: "reader_sepia",
  grayscale: "reader_grayscale",
  invert: "reader_invert",
  warmth: "reader_warmth",
  contrast: "reader_contrast",
  zoomPersist: "reader_zoom_persist",
};

/** The localStorage string for a server value. */
function toLocal(value: number | boolean): string {
  return typeof value === "boolean"
    ? value
      ? "true"
      : "false"
    : String(value);
}

const CLAMP: Record<
  Exclude<AccountPrefSlice, "invert" | "zoomPersist">,
  (v: number) => number
> = {
  brightness: clampBrightness,
  sepia: clampSepia,
  grayscale: clampGrayscale,
  warmth: clampWarmth,
  contrast: clampContrast,
};

/** The server value for a localStorage string, or `null` when the stored
 *  string is not a value of the slice's type (a tampered key is dropped
 *  rather than sent). Numbers are clamped to the slider range the way the
 *  store's loaders clamp them, so a tampered `"5"` brightness adopts as
 *  `1.5` instead of failing the whole PATCH with a 422. */
function toServer(
  slice: AccountPrefSlice,
  value: string,
): number | boolean | null {
  if (slice === "invert" || slice === "zoomPersist") {
    return value === "true" ? true : value === "false" ? false : null;
  }
  const n = Number(value);
  return value.trim() !== "" && Number.isFinite(n) ? CLAMP[slice](n) : null;
}

/** Pick the seven display slices off a `MeView` (the read page hands the
 *  reader just these so the prop stays small and serialisable). */
export function accountReaderPrefsOf(me: MeView): AccountReaderPrefs {
  return {
    reader_brightness: me.reader_brightness,
    reader_sepia: me.reader_sepia,
    reader_grayscale: me.reader_grayscale,
    reader_invert: me.reader_invert,
    reader_warmth: me.reader_warmth,
    reader_contrast: me.reader_contrast,
    reader_zoom_persist: me.reader_zoom_persist,
  };
}

/** Copy every slice the account has set over the local cache. Unset
 *  (`null`) slices are left alone — see {@link noteAccountAdoption}. */
export function mirrorAccountPrefs(view: AccountReaderPrefs): void {
  for (const slice of ACCOUNT_PREF_SLICES) {
    const v = view[FIELD_OF[slice]];
    if (v === undefined || v === null) continue;
    readerPrefSet(slice, DEFAULT_SCOPE, toLocal(v));
  }
}

/** Local values for slices the account has never set, as the PATCH body
 *  that would adopt them; `null` when there is nothing to adopt. */
export function pendingAccountAdoption(
  view: AccountReaderPrefs,
): PreferencesReq | null {
  const patch: Partial<Record<AccountField, number | boolean>> = {};
  for (const slice of ACCOUNT_PREF_SLICES) {
    const server = view[FIELD_OF[slice]];
    if (server !== undefined && server !== null) continue;
    const local = readerPrefGet(slice, DEFAULT_SCOPE);
    if (local === null) continue;
    const value = toServer(slice, local);
    if (value !== null) patch[FIELD_OF[slice]] = value;
  }
  return Object.keys(patch).length === 0 ? null : (patch as PreferencesReq);
}

/** Legacy local values waiting to ride along with the first write. */
let adoption: PreferencesReq | null = null;

/** Called once per reader open, after {@link mirrorAccountPrefs}: remember
 *  what the account lacks so the next {@link pushAccountPref} adopts it. */
export function noteAccountAdoption(view: AccountReaderPrefs): void {
  adoption = pendingAccountAdoption(view);
}

/** Forget any pending adoption (the reader opened without knowing the
 *  account's state, so nothing from an earlier open may ride along). */
export function clearAccountAdoption(): void {
  adoption = null;
}

/** The PATCH body for one slice write. */
export function patchForAccountSlice(
  slice: AccountPrefSlice,
  value: string,
): PreferencesReq {
  const v = toServer(slice, value);
  return (v === null ? {} : { [FIELD_OF[slice]]: v }) as PreferencesReq;
}

export type AccountPrefTransport = (patch: PreferencesReq) => Promise<unknown>;

let transport: AccountPrefTransport | null = null;

/** Install the network sender (the Reader does this on mount; tests
 *  install a spy). `null` turns pushes into no-ops. */
export function setAccountPrefTransport(fn: AccountPrefTransport | null): void {
  transport = fn;
}

/** How long changed slices are batched before one PATCH goes out. */
export const ACCOUNT_PUSH_COALESCE_MS = 400;

let pending: {
  patch: PreferencesReq;
  timer: ReturnType<typeof setTimeout>;
} | null = null;

/** Queue `patch`, merged with anything else changed in the window. The
 *  first push of a session also carries the adoption patch. A failed
 *  send is re-queued behind the next push (the local cache already holds
 *  the change), so an offline burst still reaches the account once a
 *  later change goes through — and the next online open adopts whatever
 *  never did. */
export function pushAccountPrefs(patch: PreferencesReq): void {
  if (!transport) return;
  const adopt = adoption;
  adoption = null;
  const merged = { ...(adopt ?? {}), ...(pending?.patch ?? {}), ...patch };
  if (pending) clearTimeout(pending.timer);
  pending = {
    patch: merged,
    timer: setTimeout(flush, ACCOUNT_PUSH_COALESCE_MS),
  };
}

/** One slice write from the store. */
export function pushAccountPref(slice: AccountPrefSlice, value: string): void {
  pushAccountPrefs(patchForAccountSlice(slice, value));
}

function flush(): void {
  const entry = pending;
  pending = null;
  if (!entry || !transport) return;
  if (Object.keys(entry.patch).length === 0) return;
  void transport(entry.patch).catch(() => {
    // Keep what failed for the next push; anything the user changed
    // since wins over the stale copy.
    adoption = { ...entry.patch, ...(adoption ?? {}) };
  });
}

/** Send whatever is queued now (tests, and the Reader's unmount). */
export function flushAccountPrefs(): void {
  if (pending) clearTimeout(pending.timer);
  flush();
}
