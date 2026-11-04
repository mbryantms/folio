/** Recent-visits list semantics (dedupe, cap, corrupt storage) plus the
 *  page → entry builders. Pure helpers; the store wrapper is covered in
 *  `tests/dom/recent-visits-store.test.tsx`. */
import { describe, expect, it } from "vitest";

import type {
  EntityDetailView,
  IssueDetailView,
  SavedViewView,
  SeriesView,
} from "@/lib/api/types";
import {
  RECENT_VISITS_MAX,
  appendVisit,
  entityVisit,
  issueVisit,
  parseVisits,
  removeVisit,
  savedViewVisit,
  seriesVisit,
  type RecentVisit,
} from "@/lib/search/recent-visits";

const v = (id: string, over: Partial<RecentVisit> = {}): RecentVisit => ({
  kind: "series",
  id,
  title: `Series ${id}`,
  subtitle: null,
  href: `/series/${id}`,
  thumbUrl: null,
  ...over,
});

describe("appendVisit", () => {
  it("puts the newest visit first", () => {
    expect(appendVisit([v("a")], v("b")).map((x) => x.id)).toEqual(["b", "a"]);
  });

  it("dedupes by kind + id, moving the entry to the front with fresh data", () => {
    const before = [v("a"), v("b"), v("c")];
    const after = appendVisit(before, v("c", { title: "Renamed" }));
    expect(after.map((x) => x.id)).toEqual(["c", "a", "b"]);
    expect(after[0]!.title).toBe("Renamed");
    // Not mutated.
    expect(before.map((x) => x.id)).toEqual(["a", "b", "c"]);
  });

  it("keeps same id under different kinds apart", () => {
    const after = appendVisit([v("x")], v("x", { kind: "issue" }));
    expect(after).toHaveLength(2);
  });

  it("caps at RECENT_VISITS_MAX, evicting the oldest", () => {
    let acc: RecentVisit[] = [];
    for (let i = 0; i < RECENT_VISITS_MAX + 3; i++)
      acc = appendVisit(acc, v(`${i}`));
    expect(acc).toHaveLength(RECENT_VISITS_MAX);
    expect(acc[0]!.id).toBe(`${RECENT_VISITS_MAX + 2}`);
    expect(acc[acc.length - 1]!.id).toBe("3");
  });

  it("rejects malformed or external-link entries", () => {
    expect(appendVisit([v("a")], v("b", { href: "//evil.example" }))).toEqual([
      v("a"),
    ]);
    expect(appendVisit([], v("b", { href: "javascript:alert(1)" }))).toEqual(
      [],
    );
    expect(appendVisit([], v("b", { title: "  " }))).toEqual([]);
  });
});

describe("removeVisit", () => {
  it("removes by kind + id only", () => {
    const list = [v("a"), v("a", { kind: "issue" }), v("b")];
    expect(removeVisit(list, { kind: "series", id: "a" })).toEqual([
      v("a", { kind: "issue" }),
      v("b"),
    ]);
  });
});

describe("parseVisits (corrupt storage)", () => {
  it("returns [] for missing, unparseable or non-array values", () => {
    expect(parseVisits(null)).toEqual([]);
    expect(parseVisits("")).toEqual([]);
    expect(parseVisits("{not json")).toEqual([]);
    expect(parseVisits('{"a":1}')).toEqual([]);
    expect(parseVisits("42")).toEqual([]);
  });

  it("drops bad entries but keeps good ones", () => {
    const raw = JSON.stringify([
      v("a"),
      null,
      "string",
      { kind: "series", id: "b" },
      v("c", { kind: "nope" as RecentVisit["kind"] }),
      v("d", { thumbUrl: 5 as unknown as string }),
      v("e", { subtitle: undefined, thumbUrl: undefined }),
    ]);
    const parsed = parseVisits(raw);
    expect(parsed.map((x) => x.id)).toEqual(["a", "e"]);
    // Optional fields normalise to null.
    expect(parsed[1]).toEqual(v("e"));
  });

  it("dedupes and caps what it reads", () => {
    const many = Array.from({ length: RECENT_VISITS_MAX + 5 }, (_, i) =>
      v(`${i}`),
    );
    const raw = JSON.stringify([v("0"), ...many]);
    const parsed = parseVisits(raw);
    expect(parsed).toHaveLength(RECENT_VISITS_MAX);
    expect(parsed.filter((x) => x.id === "0")).toHaveLength(1);
  });
});

describe("builders", () => {
  it("seriesVisit: publisher · year span · issue count", () => {
    const s = {
      id: "s1",
      slug: "saga",
      name: "Saga",
      publisher: "Image",
      earliest_year: 2012,
      latest_year: 2018,
      issue_count: 54,
      cover_url: "/c.jpg",
    } as SeriesView;
    expect(seriesVisit(s)).toEqual({
      kind: "series",
      id: "s1",
      title: "Saga",
      subtitle: "Image · 2012–2018 · 54 issues",
      href: "/series/saga",
      thumbUrl: "/c.jpg",
    });
  });

  it("issueVisit: no repeated series/number for untitled issues", () => {
    const base = {
      id: "i1",
      slug: "1",
      series_slug: "saga",
      series_name: "Saga",
      number: "1",
      year: 2012,
      state: "active",
    } as IssueDetailView;
    expect(issueVisit(base)).toMatchObject({
      title: "Saga #1",
      subtitle: "Issue · 2012",
      href: "/series/saga/issues/1",
      thumbUrl: "/issues/i1/pages/0/thumb",
    });
    expect(issueVisit({ ...base, title: "Chapter One" })).toMatchObject({
      title: "Chapter One",
      subtitle: "Saga · #1 · 2012",
    });
  });

  it("entityVisit: portrait + counts, links to the entity page", () => {
    const e = {
      id: "e1",
      slug: "storm",
      name: "Storm",
      issue_count: 1,
      series_count: 1,
      image_url: "/storm.png",
    } as EntityDetailView;
    expect(entityVisit("characters", e)).toMatchObject({
      kind: "characters",
      subtitle: "1 issue · 1 series",
      href: "/characters/storm",
      thumbUrl: "/storm.png",
    });
  });

  it("savedViewVisit: skips system rails, maps collections", () => {
    expect(
      savedViewVisit({
        id: "x",
        kind: "system",
        name: "On deck",
      } as SavedViewView),
    ).toBeNull();
    expect(
      savedViewVisit({
        id: "w",
        kind: "collection",
        name: "Want to Read",
        system_key: "want_to_read",
      } as SavedViewView),
    ).toMatchObject({ kind: "collection", href: "/views/want-to-read" });
  });
});
