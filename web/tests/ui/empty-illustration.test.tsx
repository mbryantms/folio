/**
 * Empty / 404 / error states share the static cover-silhouette
 * illustration (flair pre-merge polish): on by default, never shimmering,
 * with the state's icon riding in its corner badge.
 */
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { SearchX } from "lucide-react";
import { describe, expect, it } from "vitest";

import { EmptyState } from "@/components/folio/empty-state";
import { StatusCard, StatusErrorIcon } from "@/components/StatusScreen";

const covers = (html: string) =>
  html.split("cover-skeleton cover-silhouette").length - 1;

describe("EmptyState illustration", () => {
  it("draws three static cover silhouettes by default", () => {
    const html = renderToStaticMarkup(
      createElement(EmptyState, { size: "sm", title: "Nothing here" }),
    );
    expect(covers(html)).toBe(3);
    expect(html).not.toContain("skeleton-shimmer");
    expect(html).not.toContain('data-slot="skeleton"');
    expect(html).toContain('aria-hidden="true"');
  });

  it("puts the icon in the corner badge instead of the icon tile", () => {
    const html = renderToStaticMarkup(
      createElement(EmptyState, { icon: SearchX, title: "No matches" }),
    );
    expect(html).toContain("cover-silhouettes__badge--icon");
    expect(html).not.toContain('data-variant="icon"');
  });

  it("rail size and illustration={false} keep the plain icon tile", () => {
    for (const props of [
      { size: "rail" as const, icon: SearchX, description: "x" },
      { illustration: false, icon: SearchX, title: "x" },
    ]) {
      const html = renderToStaticMarkup(createElement(EmptyState, props));
      expect(covers(html)).toBe(0);
      expect(html).toContain('data-variant="icon"');
    }
  });
});

describe("StatusCard illustration", () => {
  it("404 shows the silhouettes above the code", () => {
    const html = renderToStaticMarkup(
      createElement(StatusCard, {
        code: "404",
        title: "Page not found",
        description: "Gone.",
        actions: null,
      }),
    );
    expect(covers(html)).toBe(3);
    expect(html.indexOf("cover-silhouettes")).toBeLessThan(html.indexOf("404"));
  });

  it("an error icon becomes the badge, rendered once", () => {
    const html = renderToStaticMarkup(
      createElement(StatusCard, {
        icon: createElement(StatusErrorIcon),
        title: "Something went wrong",
        description: "Oops.",
        actions: null,
      }),
    );
    expect(html.split("bg-destructive/10").length - 1).toBe(1);
    expect(html).toContain("cover-silhouettes__badge");
  });
});
