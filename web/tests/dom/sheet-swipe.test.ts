// @vitest-environment jsdom
/**
 * Bottom sheets dismiss on a long or fast drag down and spring back on a
 * short one (lib/ui/sheet-swipe, folio-flair pre-merge polish).
 */
import { afterEach, describe, expect, it, vi } from "vitest";

import { attachSheetSwipe } from "@/lib/ui/sheet-swipe";

function sheet(): HTMLElement {
  const el = document.createElement("div");
  el.dataset.slot = "sheet-content";
  el.dataset.side = "bottom";
  el.dataset.state = "open";
  el.innerHTML = "<p>body</p>";
  document.body.append(el);
  Object.defineProperty(el, "offsetHeight", { value: 400 });
  return el;
}

function pointer(el: Element, type: string, y: number, t: number): void {
  const e = new MouseEvent(type, { bubbles: true, clientX: 10, clientY: y });
  Object.assign(e, {
    pointerId: 1,
    pointerType: "touch",
    isPrimary: true,
  });
  Object.defineProperty(e, "timeStamp", { value: t });
  el.dispatchEvent(e);
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("sheet swipe", () => {
  it("dismisses through Escape after a long drag", () => {
    const el = sheet();
    attachSheetSwipe(el);
    const esc = vi.fn();
    document.addEventListener("keydown", (e) => esc(e.key), { once: true });
    const body = el.querySelector("p")!;
    pointer(body, "pointerdown", 100, 0);
    pointer(body, "pointermove", 150, 100);
    expect(el.style.translate).toBe("0px 50px");
    pointer(body, "pointerup", 300, 400);
    expect(esc).toHaveBeenCalledWith("Escape");
  });

  it("springs back after a short slow drag", () => {
    const el = sheet();
    attachSheetSwipe(el);
    const esc = vi.fn();
    document.addEventListener("keydown", esc, { once: true });
    const body = el.querySelector("p")!;
    pointer(body, "pointerdown", 100, 0);
    pointer(body, "pointermove", 130, 200);
    pointer(body, "pointerup", 140, 600);
    expect(esc).not.toHaveBeenCalled();
    // `translate: 0px 0px` serializes to its shortest form, `0px`.
    expect(el.style.translate).toBe("0px");
  });

  it("leaves a scrolled sheet to scroll", () => {
    const el = sheet();
    attachSheetSwipe(el);
    el.scrollTop = 50;
    const body = el.querySelector("p")!;
    pointer(body, "pointerdown", 200, 0);
    pointer(body, "pointermove", 260, 100);
    expect(el.style.translate).toBe("");
  });
});
