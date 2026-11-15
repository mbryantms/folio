/**
 * The title-bar lockup (WP-9.3): one component for the three shells, a
 * link named "Folio" whose drawings are hidden from assistive tech, with
 * the glyph and lettering matching the masters in `public/brand/`.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

vi.mock("next/link", () => ({
  default: ({
    children,
    ...props
  }: React.AnchorHTMLAttributes<HTMLAnchorElement>) => (
    <a data-next-link="" {...props}>
      {children}
    </a>
  ),
}));

import { Wordmark } from "@/components/shell/Wordmark";
import { WordmarkAnchor } from "@/components/shell/WordmarkLockup";

const brand = (file: string) =>
  readFileSync(resolve(__dirname, "../../public/brand", file), "utf8");

/** Every `d="…"` in a string of markup, in order. */
const paths = (markup: string) =>
  [...markup.matchAll(/\sd="([^"]+)"/g)].map((m) => m[1]);

describe("Wordmark", () => {
  const html = renderToStaticMarkup(<Wordmark href="/home" />);

  it("is one link named Folio with no visible text", () => {
    expect(html.match(/<a /g)).toHaveLength(1);
    expect(html).toContain('href="/home"');
    expect(html).toContain('aria-label="Folio"');
    // No text node: the name is drawn, so label-in-name cannot mismatch.
    expect(html.replace(/<[^>]+>/g, "").trim()).toBe("");
  });

  it("hides both drawings from assistive tech", () => {
    const svgs = html.match(/<svg [^>]*>/g) ?? [];
    expect(svgs).toHaveLength(2);
    for (const svg of svgs) expect(svg).toContain('aria-hidden="true"');
  });

  it("draws the glyph from the icon master, without its tile", () => {
    const [glyph] = html.split("</svg>");
    expect(glyph).toContain('viewBox="100 126 312 272"');
    expect(glyph).toContain('width="27"');
    expect(glyph).toContain('height="24"');
    expect(glyph).not.toContain("<rect");
    expect(paths(glyph!)).toEqual(paths(brand("icon-master.svg")));
    // The accent colour, so it follows the account's accent.
    expect(glyph).toContain("text-primary");
  });

  it("draws the letters from the logotype master at a 16px cap height", () => {
    const letters = html.split("</svg>")[1]!;
    expect(letters).toContain('viewBox="0 0 270 100"');
    expect(letters).toContain('height="16"');
    const master = brand("logotype-master.svg");
    expect(master).toContain('viewBox="0 0 270 100"');
    expect(paths(letters).sort()).toEqual(paths(master).sort());
    const circles = (s: string) =>
      [...s.matchAll(/<circle cx="(\d+)" cy="(\d+)" r="(\d+)"/g)]
        .map((m) => m.slice(1).join(","))
        .sort();
    expect(circles(letters)).toEqual(circles(master));
    // The dot of the "i" is the one accent-coloured piece of the name.
    expect(letters).toMatch(
      /<circle cx="182" cy="10" r="10" class="text-primary"/,
    );
  });

  it("has hover and keyboard-focus styles and the touch tap area", () => {
    expect(html).toContain("hover:opacity-80");
    expect(html).toContain("focus-visible:ring-2");
    expect(html).toContain("folio-tap");
  });

  it("the status screen's anchor is the same lockup without next/link", () => {
    expect(html).toContain("data-next-link");
    const hard = renderToStaticMarkup(<WordmarkAnchor href="/" />);
    expect(hard).not.toContain("data-next-link");
    expect(hard).toContain('aria-label="Folio"');
    expect(hard).toBe(
      html
        .replace(' data-next-link=""', "")
        .replace('href="/home"', 'href="/"'),
    );
    // The reader's first-load bundle includes the status screen (error
    // boundaries); the lockup file must stay free of router imports.
    const source = readFileSync(
      resolve(__dirname, "../../components/shell/WordmarkLockup.tsx"),
      "utf8",
    );
    expect(source).not.toMatch(/from "next\//);
  });
});

describe("the shells", () => {
  it("use the shared lockup, not a pasted text link", () => {
    for (const file of [
      "components/library/MainShell.tsx",
      "components/admin/AdminShell.tsx",
      "components/StatusScreen.tsx",
    ]) {
      const source = readFileSync(resolve(__dirname, "../..", file), "utf8");
      expect(source, file).toMatch(/<Wordmark(Anchor)? /);
      expect(source, file).not.toMatch(/>\s*Folio\s*<\/(a|Link)>/);
    }
  });
});
