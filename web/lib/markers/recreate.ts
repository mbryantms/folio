/**
 * `MarkerView → CreateMarkerReq` projection. Powers the Undo affordance
 * on marker-delete toasts (cleanup plan M3.5) — the call site captures
 * the marker snapshot before delete, then Undo recreates it from the
 * snapshot via `useCreateMarker`.
 *
 * Field-by-field mapping:
 *   - `issue_id`, `kind`, `page_index` — identity / placement
 *   - `region`, `selection`, `body`, `color` — content
 *   - `is_favorite` — flag (default false on create; explicit on undo)
 *   - `tags` — `[]` rather than `undefined` so a tagged marker round-trips
 *
 * Server-side identity is regenerated on insert; the recreated marker
 * has a new `id` and `created_at`. That's expected — Undo restores the
 * *content*, not the row.
 */
import type { CreateMarkerReq, MarkerView } from "@/lib/api/types";

export function markerToCreateReq(m: MarkerView): CreateMarkerReq {
  return {
    issue_id: m.issue_id,
    page_index: m.page_index,
    kind: m.kind,
    region: m.region ?? null,
    selection: m.selection ?? null,
    body: m.body ?? null,
    color: m.color ?? null,
    is_favorite: m.is_favorite,
    tags: m.tags,
  };
}
