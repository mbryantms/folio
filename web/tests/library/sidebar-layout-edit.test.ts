import { describe, expect, it } from "vitest";

import type { SidebarEntryView } from "@/lib/api/types";
import {
  insertAbove,
  isCustomDivider,
  keyOf,
  moveBy,
  newDivider,
  rename,
  reorder,
  showAgain,
  splitLayout,
  takeOut,
  toPayload,
} from "@/lib/sidebar-layout/edit";

function row(
  kind: SidebarEntryView["kind"],
  ref_id: string,
  label: string,
  visible = true,
): SidebarEntryView {
  return {
    kind,
    ref_id,
    label,
    icon: "",
    href: `/${ref_id}`,
    visible,
    position: 0,
  };
}

const ENTRIES = [
  row("header", "default:browse", "Browse"),
  row("builtin", "home", "Home"),
  row("builtin", "bookmarks", "Bookmarks"),
  row("header", "h-custom", "Mine"),
  row("view", "v1", "Hickman"),
  row("spacer", "s-custom", ""),
  row("builtin", "want_to_read", "Want to Read", false),
];
const state = () => splitLayout(ENTRIES);
const labels = (rows: SidebarEntryView[]) => rows.map((r) => r.label || r.kind);

describe("sidebar layout edits", () => {
  it("splits visible from hidden and keeps both in the payload", () => {
    const s = state();
    expect(s.visible).toHaveLength(6);
    expect(labels(s.hidden)).toEqual(["Want to Read"]);
    const payload = toPayload(s);
    expect(payload.map((p) => p.position)).toEqual([0, 1, 2, 3, 4, 5, 6]);
    expect(payload.at(-1)).toMatchObject({
      ref_id: "want_to_read",
      visible: false,
    });
    // A spacer has no label to send; a header always sends its own.
    expect(payload.find((p) => p.kind === "spacer")?.label).toBeNull();
    expect(payload.find((p) => p.ref_id === "h-custom")?.label).toBe("Mine");
  });

  it("drops a row onto another row's slot, across sections", () => {
    const next = reorder(state(), "view:v1", "builtin:home");
    expect(labels(next.visible)).toEqual([
      "Browse",
      "Hickman",
      "Home",
      "Bookmarks",
      "Mine",
      "spacer",
    ]);
    expect(reorder(state(), "view:v1", "view:v1")).toEqual(state());
  });

  it("steps a row and stops at the ends", () => {
    const s = state();
    expect(labels(moveBy(s, "builtin:home", 1).visible).slice(1, 3)).toEqual([
      "Bookmarks",
      "Home",
    ]);
    expect(moveBy(s, "header:default:browse", -1)).toBe(s);
    expect(moveBy(s, "spacer:s-custom", 1)).toBe(s);
  });

  it("hides ordinary rows and default headers, deletes custom dividers", () => {
    const s = state();
    expect(isCustomDivider(ENTRIES[0]!)).toBe(false);
    expect(isCustomDivider(ENTRIES[3]!)).toBe(true);
    const hid = takeOut(s, "builtin:home");
    expect(labels(hid.hidden)).toEqual(["Want to Read", "Home"]);
    expect(hid.hidden.at(-1)?.visible).toBe(false);
    const hidHeader = takeOut(s, "header:default:browse");
    expect(labels(hidHeader.hidden)).toContain("Browse");
    const gone = takeOut(s, "header:h-custom");
    expect(labels(gone.visible)).not.toContain("Mine");
    expect(gone.hidden).toEqual(s.hidden);
  });

  it("shows a hidden row again at the end", () => {
    const next = showAgain(state(), "builtin:want_to_read");
    expect(next.hidden).toEqual([]);
    expect(next.visible.at(-1)).toMatchObject({
      label: "Want to Read",
      visible: true,
    });
  });

  it("renames, ignoring a blank or unchanged name", () => {
    const s = state();
    expect(rename(s, "view:v1", "  Hickman run ").visible[4]!.label).toBe(
      "Hickman run",
    );
    expect(rename(s, "view:v1", "   ")).toBe(s);
    expect(rename(s, "view:v1", "Hickman")).toBe(s);
  });

  it("inserts a new header above a row, or at the end", () => {
    const header = newDivider("header", "h-new");
    expect(keyOf(header)).toBe("header:h-new");
    const above = insertAbove(state(), "view:v1", header);
    expect(labels(above.visible).slice(3, 6)).toEqual([
      "Mine",
      "New header",
      "Hickman",
    ]);
    const end = insertAbove(state(), null, newDivider("spacer", "s-new"));
    expect(end.visible.at(-1)?.ref_id).toBe("s-new");
  });
});
