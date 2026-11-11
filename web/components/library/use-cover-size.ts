"use client";

import * as React from "react";

import {
  COVER_MIN_VAR,
  COVER_SIZE_KEY,
  COVER_SIZE_MAX_PX,
  COVER_SIZE_MIN_PX,
  COVER_WIDTH_KEY,
  COVER_WIDTH_VAR,
  DEFAULT_COVER_SIZE_PX,
  LEGACY_LIBRARY_SIZE_KEY,
  coverColumnWidth,
  coverStops,
  coverTargetForColumns,
  parseCoverSize,
} from "@/lib/library/cover-size";
import { computeColumnsPerRow } from "@/lib/library/grid-window";
import { captureCardLayout, playCardFlip } from "@/lib/ui/card-flip";

/**
 * The app-wide cover size (lib/library/cover-size.ts): one module-level
 * store, so every grid, rail and size control on the page agrees and
 * changes in the same commit.
 *
 * - `target` is the stored preference (localStorage, per browser).
 * - `containerWidth` is the library's row width — the content box of the
 *   shell's `#main-content` column, measured by `<CoverSizeSync>` — which
 *   turns the target into a column count, the slider's stops and the
 *   real cover width rails use.
 *
 * A change made through `setColumns` / `reset` snapshots the cards first
 * (`captureCardLayout`) and the hook's layout effect plays the glide after
 * the commit; a width change (resize, sidebar toggle) does not animate.
 */
interface CoverState {
  target: number;
  containerWidth: number;
  /** Last persisted column width, used until the first measurement. */
  storedWidth: number | null;
}

const SERVER_STATE: CoverState = {
  target: DEFAULT_COVER_SIZE_PX,
  containerWidth: 0,
  storedWidth: null,
};

let state: CoverState = SERVER_STATE;
let loaded = false;
const listeners = new Set<() => void>();

function readStoredTarget(): number {
  try {
    const ls = window.localStorage;
    return (
      parseCoverSize(ls.getItem(COVER_SIZE_KEY)) ??
      parseCoverSize(ls.getItem(LEGACY_LIBRARY_SIZE_KEY)) ??
      DEFAULT_COVER_SIZE_PX
    );
  } catch {
    return DEFAULT_COVER_SIZE_PX;
  }
}

function readStoredWidth(): number | null {
  try {
    const n = Number(window.localStorage.getItem(COVER_WIDTH_KEY));
    return Number.isFinite(n) && n >= 48 && n <= 960 ? n : null;
  } catch {
    return null;
  }
}

function emit(): void {
  for (const l of listeners) l();
}

/** Mirror the state onto `<html>` (and persist the width for the
 *  pre-paint bootstrap). */
function publish(): void {
  if (typeof document === "undefined") return;
  const style = document.documentElement.style;
  style.setProperty(COVER_MIN_VAR, `${state.target}px`);
  if (state.containerWidth <= 0) return;
  const width = coverColumnWidth(state.containerWidth, state.target);
  style.setProperty(COVER_WIDTH_VAR, `${width}px`);
  try {
    window.localStorage.setItem(COVER_WIDTH_KEY, String(width));
  } catch {
    // Storage blocked: the size still applies for this page.
  }
}

function onStorage(e: StorageEvent): void {
  if (e.key !== null && e.key !== COVER_SIZE_KEY) return;
  const target = readStoredTarget();
  if (target === state.target) return;
  state = { ...state, target };
  publish();
  emit();
}

function load(): void {
  if (loaded || typeof window === "undefined") return;
  loaded = true;
  state = {
    ...state,
    target: readStoredTarget(),
    storedWidth: readStoredWidth(),
  };
  window.addEventListener("storage", onStorage);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): CoverState {
  load();
  return state;
}

function getServerSnapshot(): CoverState {
  return SERVER_STATE;
}

/** Store a new target width. Animates the cards on screen. */
export function setCoverTarget(next: number): void {
  load();
  const target = Math.min(
    COVER_SIZE_MAX_PX,
    Math.max(COVER_SIZE_MIN_PX, Math.round(next)),
  );
  if (target === state.target) return;
  captureCardLayout();
  state = { ...state, target };
  try {
    window.localStorage.setItem(COVER_SIZE_KEY, String(target));
  } catch {
    // Storage blocked: the size still applies for this page.
  }
  publish();
  emit();
}

/** Report the library row width (`#main-content`'s content box). */
export function setCoverContainerWidth(width: number): void {
  load();
  const containerWidth = Math.max(0, Math.round(width * 2) / 2);
  if (containerWidth === state.containerWidth) return;
  state = { ...state, containerWidth };
  publish();
  emit();
}

/** Tests only: back to the unloaded state. */
export function resetCoverSizeStoreForTests(): void {
  if (loaded && typeof window !== "undefined")
    window.removeEventListener("storage", onStorage);
  loaded = false;
  state = SERVER_STATE;
}

export interface CoverSize {
  /** Stored target width (the grids' `minmax` minimum), px. */
  target: number;
  /** Covers per library row; `0` until the row has been measured. */
  columns: number;
  /** The library grid's real cover width, px: what a rail card uses. */
  coverWidth: number;
  /** Column counts the slider offers, ascending. Empty until measured. */
  stops: number[];
  /** Covers per row at the default size. */
  defaultColumns: number;
  setColumns: (columns: number) => void;
  reset: () => void;
}

function setColumns(columns: number): void {
  if (state.containerWidth <= 0) return;
  setCoverTarget(coverTargetForColumns(state.containerWidth, columns));
}

function reset(): void {
  setCoverTarget(DEFAULT_COVER_SIZE_PX);
}

/** Play the glide for a size change once the new layout is committed.
 *  Every consumer runs this; `playCardFlip` only plays for the first. */
function useFlipOnChange(target: number): void {
  const flipReady = React.useRef(false);
  React.useLayoutEffect(() => {
    if (flipReady.current) playCardFlip();
    flipReady.current = true;
  }, [target]);
}

function getTarget(): number {
  load();
  return state.target;
}

function getServerTarget(): number {
  return DEFAULT_COVER_SIZE_PX;
}

/**
 * Just the stored target, for a grid that measures its own row
 * (`VirtualizedCardGrid`, the series Related tab). Unlike `useCoverSize`
 * it does not re-render when the library row is resized.
 */
export function useCoverTarget(): number {
  const target = React.useSyncExternalStore(
    subscribe,
    getTarget,
    getServerTarget,
  );
  useFlipOnChange(target);
  return target;
}

/**
 * The library grid's cover width right now, px. For imperative math only
 * (scroll compensation in an effect or handler); to *size* a card use
 * `COVER_WIDTH`, which needs no render.
 */
export function getCoverWidth(): number {
  load();
  return state.containerWidth > 0
    ? coverColumnWidth(state.containerWidth, state.target)
    : (state.storedWidth ?? state.target);
}

export function useCoverSize(): CoverSize {
  const snap = React.useSyncExternalStore(
    subscribe,
    getSnapshot,
    getServerSnapshot,
  );
  useFlipOnChange(snap.target);
  return React.useMemo(() => {
    const { target, containerWidth, storedWidth } = snap;
    const measured = containerWidth > 0;
    const columns = measured ? computeColumnsPerRow(containerWidth, target) : 0;
    return {
      target,
      columns,
      coverWidth: measured
        ? coverColumnWidth(containerWidth, target)
        : (storedWidth ?? target),
      stops: measured ? coverStops(containerWidth, columns) : [],
      defaultColumns: measured
        ? computeColumnsPerRow(containerWidth, DEFAULT_COVER_SIZE_PX)
        : 0,
      setColumns,
      reset,
    };
  }, [snap]);
}

/**
 * Measures the library row — the content box of the shell's
 * `#main-content` column — and keeps the store current. Mounted once by
 * `MainShell`; renders nothing.
 */
export function CoverSizeSync(): null {
  React.useLayoutEffect(() => {
    const el = document.getElementById("main-content");
    if (!el) return;
    const cs = window.getComputedStyle(el);
    const pad =
      (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
    setCoverContainerWidth(el.clientWidth - pad);
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (typeof w === "number") setCoverContainerWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return null;
}
