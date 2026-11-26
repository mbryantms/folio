// @vitest-environment jsdom
/**
 * Where a pull-to-refresh may start. A touch inside a layer over the page
 * (the phone nav sheet, a dialog, a menu) or inside a scrolled container
 * must not pull the page behind it.
 */
import { afterEach, describe, expect, it } from "vitest";

import { canStartPull } from "@/lib/use-pull-to-refresh";

function mount(html: string): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = html;
  document.body.appendChild(host);
  return host;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("canStartPull", () => {
  it("allows a touch on ordinary page content", () => {
    const host = mount(`<main><p id="t">Releases</p></main>`);
    expect(canStartPull(host.querySelector("#t"))).toBe(true);
    expect(canStartPull(null)).toBe(true);
  });

  it("refuses a touch inside a sheet, dialog, menu or popover", () => {
    for (const wrapper of [
      `<div role="dialog" data-sidebar="sidebar"><a id="t" href="/">Back to library</a></div>`,
      `<div role="alertdialog"><button id="t">Delete</button></div>`,
      `<div role="menu"><div id="t" role="menuitem">Edit</div></div>`,
      `<div data-radix-popper-content-wrapper><div id="t">Pick</div></div>`,
    ]) {
      const host = mount(wrapper);
      expect(canStartPull(host.querySelector("#t")), wrapper).toBe(false);
    }
  });

  it("refuses a touch inside a container that is scrolled down", () => {
    const host = mount(`<div id="scroller"><p id="t">Row</p></div>`);
    const scroller = host.querySelector("#scroller") as HTMLElement;
    expect(canStartPull(host.querySelector("#t"))).toBe(true);
    scroller.scrollTop = 40;
    expect(canStartPull(host.querySelector("#t"))).toBe(false);
  });
});
