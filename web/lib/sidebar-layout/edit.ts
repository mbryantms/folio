/**
 * Pure edits of the sidebar layout (inline sidebar editing).
 *
 * The server stores the layout as one flat ordered list: built-ins,
 * libraries, saved views and pages, with `header` and `spacer` rows as
 * the section boundaries. A save replaces the whole list, so every edit
 * here returns the full next state — the visible rows in order, then the
 * hidden ones — and `toPayload` turns that into the request.
 */
import type { SidebarEntryView, UpdateEntryReq } from "@/lib/api/types";

export type LayoutState = {
  /** Rows shown in the sidebar, in order. */
  visible: SidebarEntryView[];
  /** Rows the user hid; kept so they can be shown again. */
  hidden: SidebarEntryView[];
};

export function keyOf(e: Pick<SidebarEntryView, "kind" | "ref_id">): string {
  return `${e.kind}:${e.ref_id}`;
}

export function splitLayout(entries: readonly SidebarEntryView[]): LayoutState {
  return {
    visible: entries.filter((e) => e.visible),
    hidden: entries.filter((e) => !e.visible),
  };
}

/** A header or spacer the user made. Removing one deletes it; every
 *  other row (and the default headers) is hidden instead, so it can come
 *  back. */
export function isCustomDivider(e: SidebarEntryView): boolean {
  return (
    (e.kind === "header" || e.kind === "spacer") &&
    !e.ref_id.startsWith("default:")
  );
}

/** The whole-list request: dense positions, hidden rows after the
 *  visible ones (leaving them out would un-hide them), labels kept. */
export function toPayload(state: LayoutState): UpdateEntryReq[] {
  return [...state.visible, ...state.hidden].map((e, idx) => ({
    kind: e.kind,
    ref_id: e.ref_id,
    visible: e.visible,
    position: idx,
    label: e.kind === "header" || e.label ? e.label : null,
  }));
}

function indexOfKey(rows: readonly SidebarEntryView[], key: string): number {
  return rows.findIndex((e) => keyOf(e) === key);
}

/** Move a visible row to another visible row's slot (a drop). */
export function reorder(
  state: LayoutState,
  fromKey: string,
  toKey: string,
): LayoutState {
  const from = indexOfKey(state.visible, fromKey);
  const to = indexOfKey(state.visible, toKey);
  if (from < 0 || to < 0 || from === to) return state;
  const visible = [...state.visible];
  const [row] = visible.splice(from, 1);
  visible.splice(to, 0, row!);
  return { ...state, visible };
}

/** Move a visible row `delta` places (clamped). */
export function moveBy(
  state: LayoutState,
  key: string,
  delta: number,
): LayoutState {
  const from = indexOfKey(state.visible, key);
  if (from < 0) return state;
  const to = Math.max(0, Math.min(state.visible.length - 1, from + delta));
  if (to === from) return state;
  const visible = [...state.visible];
  const [row] = visible.splice(from, 1);
  visible.splice(to, 0, row!);
  return { ...state, visible };
}

/** Take a row out of the sidebar: a custom header or spacer is deleted,
 *  anything else moves to the hidden list. */
export function takeOut(state: LayoutState, key: string): LayoutState {
  const idx = indexOfKey(state.visible, key);
  if (idx < 0) return state;
  const row = state.visible[idx]!;
  const visible = state.visible.filter((_, i) => i !== idx);
  if (isCustomDivider(row)) return { ...state, visible };
  return { visible, hidden: [...state.hidden, { ...row, visible: false }] };
}

/** Bring a hidden row back, at the end of the sidebar. */
export function showAgain(state: LayoutState, key: string): LayoutState {
  const idx = indexOfKey(state.hidden, key);
  if (idx < 0) return state;
  const row = state.hidden[idx]!;
  return {
    visible: [...state.visible, { ...row, visible: true }],
    hidden: state.hidden.filter((_, i) => i !== idx),
  };
}

/** Rename a visible row; a blank name changes nothing. */
export function rename(
  state: LayoutState,
  key: string,
  label: string,
): LayoutState {
  const trimmed = label.trim();
  if (trimmed.length === 0) return state;
  const idx = indexOfKey(state.visible, key);
  if (idx < 0 || state.visible[idx]!.label === trimmed) return state;
  return {
    ...state,
    visible: state.visible.map((e, i) =>
      i === idx ? { ...e, label: trimmed } : e,
    ),
  };
}

/** A new header or spacer row. */
export function newDivider(
  kind: "header" | "spacer",
  refId: string,
): SidebarEntryView {
  return {
    kind,
    ref_id: refId,
    label: kind === "header" ? "New header" : "",
    icon: "",
    href: "",
    visible: true,
    position: 0,
  };
}

/** Put `row` just above the visible row `key` (at the end when `key` is
 *  not found). */
export function insertAbove(
  state: LayoutState,
  key: string | null,
  row: SidebarEntryView,
): LayoutState {
  const at = key === null ? -1 : indexOfKey(state.visible, key);
  const visible = [...state.visible];
  visible.splice(at < 0 ? visible.length : at, 0, row);
  return { ...state, visible };
}
