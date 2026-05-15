import { describe, expect, it } from "vitest";

import {
  GLOBAL_KEYBIND_ACTIONS,
  KEYBIND_DEFAULTS,
  KEYBIND_LABELS,
  KEYBIND_SCOPES,
  READER_KEYBIND_ACTIONS,
  actionForKey,
  findConflict,
  formatKey,
  resolveKeybinds,
  shouldSkipHotkey,
} from "@/lib/reader/keybinds";

describe("resolveKeybinds", () => {
  it("returns the defaults when no overrides are provided", () => {
    expect(resolveKeybinds(undefined)).toEqual(KEYBIND_DEFAULTS);
    expect(resolveKeybinds(null)).toEqual(KEYBIND_DEFAULTS);
    expect(resolveKeybinds({})).toEqual(KEYBIND_DEFAULTS);
  });

  it("merges overrides on top of defaults", () => {
    const merged = resolveKeybinds({ nextPage: "j", prevPage: "k" });
    expect(merged.nextPage).toBe("j");
    expect(merged.prevPage).toBe("k");
    // Untouched actions retain defaults.
    expect(merged.toggleChrome).toBe(KEYBIND_DEFAULTS.toggleChrome);
  });

  it("ignores unknown actions in the override map", () => {
    const merged = resolveKeybinds({ nonsense: "x" } as Record<string, string>);
    expect(merged).toEqual(KEYBIND_DEFAULTS);
  });

  it("ignores empty-string overrides", () => {
    const merged = resolveKeybinds({ nextPage: "" });
    expect(merged.nextPage).toBe(KEYBIND_DEFAULTS.nextPage);
  });
});

describe("formatKey", () => {
  it("uppercases single-character letters", () => {
    expect(formatKey("f")).toBe("F");
    expect(formatKey("a")).toBe("A");
  });
  it("renders arrows and special tokens", () => {
    expect(formatKey("ArrowLeft")).toBe("←");
    expect(formatKey("ArrowRight")).toBe("→");
    expect(formatKey("Escape")).toBe("Esc");
    expect(formatKey(" ")).toBe("Space");
    expect(formatKey("Space")).toBe("Space");
  });
  it("returns a placeholder for empty input", () => {
    expect(formatKey("")).toBe("—");
  });
});

describe("actionForKey", () => {
  it("matches default bindings", () => {
    const bindings = resolveKeybinds(undefined);
    expect(actionForKey("ArrowRight", bindings)).toBe("nextPage");
    expect(actionForKey("ArrowLeft", bindings)).toBe("prevPage");
    expect(actionForKey("Escape", bindings)).toBe("quitReader");
  });

  it("matches single-character keys case-insensitively", () => {
    const bindings = resolveKeybinds(undefined);
    expect(actionForKey("f", bindings)).toBe("cycleFit");
    expect(actionForKey("F", bindings)).toBe("cycleFit");
  });

  it("returns null for unbound keys", () => {
    const bindings = resolveKeybinds(undefined);
    expect(actionForKey("z", bindings)).toBeNull();
  });

  it("respects user overrides", () => {
    const bindings = resolveKeybinds({ nextPage: "j", prevPage: "k" });
    expect(actionForKey("j", bindings)).toBe("nextPage");
    expect(actionForKey("k", bindings)).toBe("prevPage");
    // The default `ArrowRight` is now unbound for `nextPage`.
    expect(actionForKey("ArrowRight", bindings)).toBeNull();
  });
});

describe("toggleSidebar registry entry", () => {
  it("is registered as a global-scoped action with a Mod+b default", () => {
    expect(GLOBAL_KEYBIND_ACTIONS).toContain("toggleSidebar");
    expect(KEYBIND_SCOPES.toggleSidebar).toBe("global");
    expect(KEYBIND_DEFAULTS.toggleSidebar).toBe("Mod+b");
  });

  it("dispatches via actionForKey on Ctrl+B", () => {
    const bindings = resolveKeybinds(undefined);
    expect(
      actionForKey({ key: "b", ctrlKey: true }, bindings),
    ).toBe("toggleSidebar");
  });
});

describe("nextIssue registry entry", () => {
  // Smoke checks for the M2 keybind. The Settings → Keybinds editor
  // auto-derives its list from READER_KEYBIND_ACTIONS + KEYBIND_LABELS,
  // so a presence-check here is enough to guarantee the action shows
  // up there with the right human label and default chord.
  it("is registered as a reader-scoped action with Shift+N default", () => {
    expect(READER_KEYBIND_ACTIONS).toContain("nextIssue");
    expect(KEYBIND_SCOPES.nextIssue).toBe("reader");
    expect(KEYBIND_DEFAULTS.nextIssue).toBe("Shift+N");
    expect(KEYBIND_LABELS.nextIssue).toBe("Next issue");
  });

  it("dispatches via actionForKey on Shift+N", () => {
    const bindings = resolveKeybinds(undefined);
    expect(
      actionForKey({ key: "N", shiftKey: true }, bindings),
    ).toBe("nextIssue");
  });
});

describe("prevIssue registry entry", () => {
  // Sibling to nextIssue (D-7). Same auto-derivation pattern for the
  // editor UI; presence + chord assertion is the contract.
  it("is registered as a reader-scoped action with Shift+P default", () => {
    expect(READER_KEYBIND_ACTIONS).toContain("prevIssue");
    expect(KEYBIND_SCOPES.prevIssue).toBe("reader");
    expect(KEYBIND_DEFAULTS.prevIssue).toBe("Shift+P");
    expect(KEYBIND_LABELS.prevIssue).toBe("Previous issue");
  });

  it("dispatches via actionForKey on Shift+P", () => {
    const bindings = resolveKeybinds(undefined);
    expect(
      actionForKey({ key: "P", shiftKey: true }, bindings),
    ).toBe("prevIssue");
  });
});

describe("findConflict", () => {
  it("returns null when the chord is unbound", () => {
    const resolved = resolveKeybinds(undefined);
    expect(findConflict("z", "nextPage", resolved)).toBeNull();
  });

  it("returns null when the only match is the action being edited", () => {
    // Re-binding `nextPage` to its current `ArrowRight` should not flag
    // a conflict with itself.
    const resolved = resolveKeybinds(undefined);
    expect(findConflict("ArrowRight", "nextPage", resolved)).toBeNull();
  });

  it("finds same-scope reader collisions", () => {
    // `b` is the default for `bookmarkPage`. Trying to bind `nextPage`
    // to `b` should report `bookmarkPage` as the conflict.
    const resolved = resolveKeybinds(undefined);
    expect(findConflict("b", "nextPage", resolved)).toBe("bookmarkPage");
  });

  it("finds cross-scope collisions (global vs reader)", () => {
    // Bind a reader action to a global default — collision counts
    // because the global dispatcher fires inside the reader too.
    const resolved = resolveKeybinds(undefined);
    expect(findConflict("Mod+k", "addNote", resolved)).toBe("openSearch");
  });

  it("returns null for empty / invalid chords", () => {
    const resolved = resolveKeybinds(undefined);
    expect(findConflict("", "nextPage", resolved)).toBeNull();
  });

  it("respects modifier matching", () => {
    // Bare `b` shouldn't collide with `Mod+b` (sidebar toggle).
    const resolved = resolveKeybinds(undefined);
    expect(findConflict("b", "addNote", resolved)).toBe("bookmarkPage");
    // And `Mod+b` shouldn't be flagged as colliding with bare `b`.
    expect(findConflict("Mod+b", "toggleSidebar", resolved)).toBeNull();
  });
});

describe("shouldSkipHotkey", () => {
  // Behavioral DOM checks (input / textarea / contenteditable / closest
  // semantics) live under manual smoke verification — vitest runs in
  // node-env here, so `HTMLElement` is undefined and the helper short-
  // circuits to `false`. We still verify it doesn't throw in node and
  // that the SSR guard works as documented.
  it("returns false in a node environment (SSR guard)", () => {
    const ev = { target: { tagName: "INPUT" } } as unknown as KeyboardEvent;
    expect(shouldSkipHotkey(ev)).toBe(false);
  });

  it("returns false when the event target is null", () => {
    const ev = { target: null } as unknown as KeyboardEvent;
    expect(shouldSkipHotkey(ev)).toBe(false);
  });
});
