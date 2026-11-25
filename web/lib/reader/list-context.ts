/**
 * Reader list context — the "Issue N of M in {list}" sub-label the
 * end-of-issue card and the webtoon footer show under the next-up target
 * when the resolver followed a list (a CBL reading list via `?cbl=`, or
 * a user collection via `?collection=`) rather than the parent series.
 * One helper so both surfaces word it identically.
 */
import { apiMutate } from "@/lib/api/mutations/_core";
import type { ListViewRef, NextUpView } from "@/lib/api/types";
import { apiPath } from "@/lib/api/path";

export function nextUpListSubtitle(
  data: NextUpView | undefined,
): string | null {
  if (!data) return null;
  // Linked reading lists: the target is in another list.
  const sw = data.switch_to;
  if (sw) {
    const name = displayListName(sw.name);
    return sw.reason === "return" ? `Back to ${name}` : `Continue in ${name}`;
  }
  if (
    data.source === "cbl" &&
    data.cbl_position &&
    data.cbl_total &&
    data.cbl_list_name
  ) {
    return `Issue ${data.cbl_position} of ${data.cbl_total} in ${displayListName(data.cbl_list_name)}`;
  }
  if (
    data.source === "collection" &&
    data.collection_position &&
    data.collection_total &&
    data.collection_name
  ) {
    return `Issue ${data.collection_position} of ${data.collection_total} in ${data.collection_name}`;
  }
  return null;
}

/**
 * Reading-list names carry curation tags: a leading "[Publisher]" and a
 * trailing all-caps release source like "(WEB-CBRO)". Drop both for
 * display; years like "(2016)" stay.
 */
export function displayListName(name: string): string {
  const cleaned = name
    .replace(/^\s*\[[^\]]+\]\s*/, "")
    .replace(/\s*\([A-Z][A-Z0-9-]*\)\s*$/, "")
    .trim();
  return cleaned || name;
}

/**
 * The reader's view of a reading list, for `?cbl=`: `viewId` when they
 * have one, else made on the spot (`POST /me/cbl-lists/{id}/view`).
 * `null` when that fails — the caller reads without list context.
 */
export async function ensureListView(
  listId: string,
  viewId: string | null | undefined,
): Promise<string | null> {
  if (viewId) return viewId;
  try {
    const ref = await apiMutate<ListViewRef>({
      path: apiPath`/me/cbl-lists/${listId}/view`,
      method: "POST",
    });
    return ref?.view_id ?? null;
  } catch {
    return null;
  }
}
