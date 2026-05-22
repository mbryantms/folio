import { describe, expect, it } from "vitest";

import { renderSearchSnippet } from "@/lib/search/render-snippet";

describe("renderSearchSnippet", () => {
  it("returns empty string for empty input", () => {
    expect(renderSearchSnippet("")).toBe("");
  });

  it("preserves <mark> tags around matched terms", () => {
    expect(renderSearchSnippet("a <mark>match</mark> here")).toBe(
      "a <mark>match</mark> here",
    );
  });

  it("escapes stray angle brackets in the surrounding text", () => {
    // A user-supplied summary that happens to include `<` / `>`. The
    // sanitiser must escape them so they don't smuggle in a script tag.
    expect(
      renderSearchSnippet(
        "summary with <script>alert(1)</script> and a real <mark>match</mark>",
      ),
    ).toBe(
      "summary with &lt;script&gt;alert(1)&lt;/script&gt; and a real <mark>match</mark>",
    );
  });

  it("escapes ampersands + quotes", () => {
    expect(renderSearchSnippet('Foo & "Bar"')).toBe(
      "Foo &amp; &quot;Bar&quot;",
    );
  });

  it("normalises tag casing to lowercase", () => {
    // ts_headline emits lowercase, but a defensive lowercase keeps the
    // output canonical regardless of what's upstream.
    expect(renderSearchSnippet("hit <MARK>word</MARK>")).toBe(
      "hit <mark>word</mark>",
    );
  });

  it("strips other tag-shaped substrings", () => {
    // Anything that looks like HTML but isn't an allowlisted <mark>
    // gets escaped. No other tag survives.
    expect(renderSearchSnippet("<div>hi</div>")).toBe(
      "&lt;div&gt;hi&lt;/div&gt;",
    );
  });

  it("escapes single-quote / apostrophe", () => {
    expect(renderSearchSnippet("it's a <mark>match</mark>")).toBe(
      "it&#39;s a <mark>match</mark>",
    );
  });

  it("handles multiple <mark> spans in one snippet", () => {
    expect(
      renderSearchSnippet("<mark>foo</mark> and <mark>bar</mark>"),
    ).toBe("<mark>foo</mark> and <mark>bar</mark>");
  });
});
