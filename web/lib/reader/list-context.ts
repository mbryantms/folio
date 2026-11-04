/**
 * Reader list context — the "Issue N of M in {list}" sub-label the
 * end-of-issue card and the webtoon footer show under the next-up target
 * when the resolver followed a list (a CBL reading list via `?cbl=`, or
 * a user collection via `?collection=`) rather than the parent series.
 * One helper so both surfaces word it identically.
 */
import type { NextUpView } from "@/lib/api/types";

export function nextUpListSubtitle(
  data: NextUpView | undefined,
): string | null {
  if (!data) return null;
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
