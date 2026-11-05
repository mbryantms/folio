// @vitest-environment jsdom
/** Palette row internals: the typed text is marked in the title, the meta
 *  line stays, and a backend snippet renders sanitised under it. */
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  SearchHitRowContent,
  VisitRowContent,
} from "@/components/search/SearchResultRow";

describe("SearchHitRowContent", () => {
  it("marks the query in the title and keeps meta + sanitised snippet", () => {
    const { container } = render(
      <SearchHitRowContent
        query="saga"
        hit={{
          kind: "series",
          id: "s1",
          title: "Saga",
          subtitle: "Image · 2012 · 54 issues",
          href: "/series/saga",
          thumbUrl: "/c.jpg",
          snippet: "a <mark>saga</mark> <img src=x onerror=alert(1)>",
        }}
      />,
    );
    const marks = [...container.querySelectorAll("mark")].map(
      (m) => m.textContent,
    );
    expect(marks).toEqual(["Saga", "saga"]);
    expect(container.textContent).toContain("Image · 2012 · 54 issues");
    expect(container.querySelector("img[onerror]")).toBeNull();
    // Cover is decorative; the title names the row.
    expect(container.querySelector("img")!.getAttribute("alt")).toBe("");
  });

  it("renders an icon tile when there is no cover", () => {
    const { container } = render(
      <VisitRowContent
        visit={{
          kind: "creator",
          id: "c1",
          title: "Brian K. Vaughan",
          subtitle: "Writer · 120 credits",
          href: "/creators/bkv",
        }}
      />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg")).not.toBeNull();
    expect(container.querySelector("mark")).toBeNull();
  });
});
