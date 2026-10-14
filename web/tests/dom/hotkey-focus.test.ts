// @vitest-environment jsdom
/**
 * Post-audit T2.5: window-level hotkey dispatchers (reader keymap,
 * global shortcuts) yield to a focused control's own activation keys
 * and to strokes another handler already claimed.
 */
import { describe, expect, it } from "vitest";

import { shouldSkipHotkey } from "@/lib/reader/keybinds";

function keyOn(
  el: Element | null,
  key: string,
  init: Partial<KeyboardEventInit> & { defaultPrevented?: boolean } = {},
): KeyboardEvent {
  const ev = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...init,
  });
  Object.defineProperty(ev, "target", { value: el });
  if (init.defaultPrevented) ev.preventDefault();
  return ev;
}

describe("shouldSkipHotkey", () => {
  it("skips a stroke another handler already claimed", () => {
    const ev = keyOn(document.body, "ArrowRight", { defaultPrevented: true });
    expect(shouldSkipHotkey(ev)).toBe(true);
  });

  it("skips while an IME composition is in progress", () => {
    const ev = keyOn(document.body, "n", { isComposing: true });
    expect(shouldSkipHotkey(ev)).toBe(true);
  });

  it("skips typing inside text inputs", () => {
    const input = document.createElement("input");
    document.body.append(input);
    expect(shouldSkipHotkey(keyOn(input, "b"))).toBe(true);
    expect(shouldSkipHotkey(keyOn(input, "ArrowRight"))).toBe(true);
  });

  it("leaves Space / Enter to a focused button but keeps arrows global", () => {
    const button = document.createElement("button");
    document.body.append(button);
    expect(shouldSkipHotkey(keyOn(button, " "))).toBe(true);
    expect(shouldSkipHotkey(keyOn(button, "Enter"))).toBe(true);
    expect(shouldSkipHotkey(keyOn(button, "ArrowRight"))).toBe(false);
    expect(shouldSkipHotkey(keyOn(button, "b"))).toBe(false);
  });

  it("ignores tabindex=-1 chrome (the reader's tap zones)", () => {
    const zone = document.createElement("button");
    zone.tabIndex = -1;
    document.body.append(zone);
    // A real <button> still owns Space even with tabindex=-1 — only the
    // generic tabindex clause is excluded.
    const div = document.createElement("div");
    div.tabIndex = -1;
    document.body.append(div);
    expect(shouldSkipHotkey(keyOn(div, " "))).toBe(false);
    const focusable = document.createElement("div");
    focusable.tabIndex = 0;
    document.body.append(focusable);
    expect(shouldSkipHotkey(keyOn(focusable, " "))).toBe(true);
  });

  it("leaves arrow keys to sliders and menus", () => {
    const slider = document.createElement("span");
    slider.setAttribute("role", "slider");
    document.body.append(slider);
    expect(shouldSkipHotkey(keyOn(slider, "ArrowRight"))).toBe(true);
    expect(shouldSkipHotkey(keyOn(slider, "Home"))).toBe(true);
    const menu = document.createElement("div");
    menu.setAttribute("role", "menu");
    const item = document.createElement("div");
    item.setAttribute("role", "menuitem");
    menu.append(item);
    document.body.append(menu);
    expect(shouldSkipHotkey(keyOn(item, "ArrowDown"))).toBe(true);
    expect(shouldSkipHotkey(keyOn(item, "Enter"))).toBe(true);
  });

  it("stays global on the document body", () => {
    expect(shouldSkipHotkey(keyOn(document.body, " "))).toBe(false);
    expect(shouldSkipHotkey(keyOn(document.body, "ArrowRight"))).toBe(false);
  });
});
