// @vitest-environment jsdom
/**
 * Card sizes as CSS variables (lib/library/card-size.ts): the pre-paint
 * script mirrors stored sizes onto <html>, the custom-page slug alias
 * resolves, stale variables are dropped, and grids/skeletons read them.
 */
import { beforeEach, describe, expect, it } from "vitest";

import {
  CARD_SIZE_BOOTSTRAP,
  CARD_SIZE_KEYS,
  cardGridColumns,
  cardSizeVarName,
  cardWidth,
  PAGE_SLUG_ID_PREFIX,
  syncCardSizeVars,
} from "@/lib/library/card-size";
import { cardSizeKeyForPath } from "@/components/skeletons/RouteCardGrid";

const root = () => document.documentElement.style;
const v = (key: string) => root().getPropertyValue(cardSizeVarName(key));

describe("card-size variables", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute("style");
  });

  it("names variables from storage keys", () => {
    expect(cardSizeVarName("folio.series.cardSize")).toBe(
      "--card-size-series-cardSize",
    );
    expect(cardSizeVarName(CARD_SIZE_KEYS.pageSlug("x y"))).toBe(
      "--card-size-page-cardSize-slug-x-y",
    );
  });

  it("mirrors stored sizes and ignores junk", () => {
    localStorage.setItem(CARD_SIZE_KEYS.series, "220");
    localStorage.setItem(CARD_SIZE_KEYS.library, "abc");
    localStorage.setItem(CARD_SIZE_KEYS.search, "5000");
    localStorage.setItem("unrelated", "200");
    syncCardSizeVars();
    expect(v(CARD_SIZE_KEYS.series)).toBe("220px");
    expect(v(CARD_SIZE_KEYS.library)).toBe("");
    expect(v(CARD_SIZE_KEYS.search)).toBe("");
  });

  it("publishes a custom page's size under its slug", () => {
    localStorage.setItem(CARD_SIZE_KEYS.page("p1"), "240");
    localStorage.setItem(`${PAGE_SLUG_ID_PREFIX}marvel`, "p1");
    syncCardSizeVars();
    expect(v(CARD_SIZE_KEYS.page("p1"))).toBe("240px");
    expect(v(CARD_SIZE_KEYS.pageSlug("marvel"))).toBe("240px");
  });

  it("drops variables whose keys are gone", () => {
    localStorage.setItem(CARD_SIZE_KEYS.home, "200");
    syncCardSizeVars();
    expect(v(CARD_SIZE_KEYS.home)).toBe("200px");
    localStorage.clear();
    syncCardSizeVars();
    expect(v(CARD_SIZE_KEYS.home)).toBe("");
  });

  it("runs standalone as the inline bootstrap", () => {
    localStorage.setItem(CARD_SIZE_KEYS.savedView, "180");
    // Same as the browser running the <script> body: no module scope.
    new Function(CARD_SIZE_BOOTSTRAP)();
    expect(v(CARD_SIZE_KEYS.savedView)).toBe("180px");
  });

  it("builds grid templates that fall back to the default", () => {
    expect(cardWidth(undefined)).toBe("160px");
    expect(cardWidth(CARD_SIZE_KEYS.series, 200)).toBe(
      "var(--card-size-series-cardSize, 200px)",
    );
    expect(cardGridColumns(CARD_SIZE_KEYS.series)).toBe(
      "repeat(auto-fill, minmax(min(var(--card-size-series-cardSize, 160px), calc(50% - 0.5rem)), 1fr))",
    );
  });
});

describe("cardSizeKeyForPath", () => {
  it("picks the grid's surface from the URL", () => {
    expect(cardSizeKeyForPath("/")).toBe(CARD_SIZE_KEYS.library);
    expect(
      cardSizeKeyForPath("/views/019e1940-aa60-7a73-b662-2826eac37dde"),
    ).toBe(CARD_SIZE_KEYS.savedView);
    expect(cardSizeKeyForPath("/views/want-to-read")).toBe(
      CARD_SIZE_KEYS.savedView,
    );
    expect(cardSizeKeyForPath("/views/continue-reading")).toBe(
      CARD_SIZE_KEYS.systemView,
    );
  });
});
