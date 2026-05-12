import { describe, expect, it } from "vitest";

import {
  KEYBIND_DEFAULTS,
  actionForKey,
  formatKey,
  resolveKeybinds,
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
