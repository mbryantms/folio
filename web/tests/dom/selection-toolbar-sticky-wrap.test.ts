/**
 * `SelectionToolbar`'s sticky box must be the outer presence wrapper.
 * A sticky element only moves within its parent; the inner wrapper's
 * parent is the grid-row wrapper, which is exactly the toolbar's own
 * height, so `position: sticky` there (or on the toolbar body) could
 * never engage and the toolbar scrolled off with the page. The phone
 * controls bar and the desktop library toolbar both hand the band to
 * this toolbar in select mode, so it has to actually stick.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

function block(css: string, selector: string): string {
  const at = css.indexOf(`\n${selector} {`);
  expect(at, selector).toBeGreaterThan(0);
  return css.slice(at, css.indexOf("\n}\n", at));
}

describe("selection toolbar sticky wrapper", () => {
  const css = readFileSync(join(process.cwd(), "styles/globals.css"), "utf8");

  it("makes the outer grid wrapper the sticky box", () => {
    const outer = block(css, ".selection-toolbar-wrap");
    expect(outer).toMatch(/position:\s*sticky/);
    expect(outer).toMatch(/top:\s*var\(--topbar-h\)/);
  });

  it("leaves the clipping inner wrapper in flow", () => {
    const inner = block(css, ".selection-toolbar-wrap-inner");
    expect(inner).toMatch(/overflow:\s*hidden/);
    expect(inner).not.toMatch(/position:\s*sticky/);
  });
});
