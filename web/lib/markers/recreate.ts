/**
 * `MarkerView → RestoreMarkerItem` projection. Powers the Undo
 * affordance on marker-delete toasts: the call site captures the marker
 * snapshot(s) before delete, then Undo sends them all to
 * `POST /me/markers/restore` in one request via `useRestoreMarkers`
 * (WP-8.4; it used to be one `POST /me/markers` per marker).
 *
 * Every field the server keeps is carried, so Undo restores the *row*,
 * not just its content: the original `id` (links to `/markers/{id}`
 * keep working), `created_at`, the WP-6.2 `page_hash` anchor and the
 * reading-log `hidden_from_log` flag, plus
 * placement, content, colour, favourite flag and tags.
 */
import type { MarkerView, RestoreMarkerItem } from "@/lib/api/types";

export function markerToRestoreItem(m: MarkerView): RestoreMarkerItem {
  return {
    id: m.id,
    issue_id: m.issue_id,
    page_index: m.page_index,
    kind: m.kind,
    region: m.region ?? null,
    selection: m.selection ?? null,
    body: m.body ?? null,
    color: m.color ?? null,
    is_favorite: m.is_favorite,
    tags: m.tags,
    page_hash: m.page_hash ?? null,
    hidden_from_log: m.hidden_from_log,
    created_at: m.created_at,
  };
}
