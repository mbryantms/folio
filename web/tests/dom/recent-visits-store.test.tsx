// @vitest-environment jsdom
/** The recent-visits store: same-tab updates reach a mounted reader (the
 *  search UI stays mounted while detail pages record), corrupt storage
 *  degrades to empty, and clear empties storage. */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { RecordVisit } from "@/components/search/RecordVisit";
import {
  RECENT_VISITS_STORAGE_KEY,
  type RecentVisit,
} from "@/lib/search/recent-visits";
import {
  __resetRecentVisitsStoreForTests,
  recordVisit,
  useRecentVisits,
} from "@/lib/search/use-recent-visits";

const visit = (id: string): RecentVisit => ({
  kind: "series",
  id,
  title: `Series ${id}`,
  subtitle: null,
  href: `/series/${id}`,
  thumbUrl: null,
});

function Reader() {
  const { visits, clear } = useRecentVisits();
  return (
    <>
      <p data-testid="out">{visits.map((v) => v.id).join(",")}</p>
      <button type="button" onClick={clear}>
        Clear
      </button>
    </>
  );
}

beforeEach(() => {
  localStorage.clear();
  __resetRecentVisitsStoreForTests();
});
afterEach(() => vi.restoreAllMocks());

describe("recent visits store", () => {
  it("a RecordVisit mount shows up in an already-mounted reader", () => {
    render(<Reader />);
    expect(screen.getByTestId("out").textContent).toBe("");
    render(<RecordVisit visit={visit("a")} />);
    expect(screen.getByTestId("out").textContent).toBe("a");
    act(() => recordVisit(visit("b")));
    act(() => recordVisit(visit("a")));
    expect(screen.getByTestId("out").textContent).toBe("a,b");
    expect(
      JSON.parse(localStorage.getItem(RECENT_VISITS_STORAGE_KEY)!).map(
        (v: RecentVisit) => v.id,
      ),
    ).toEqual(["a", "b"]);
  });

  it("corrupt storage reads as empty and is overwritten on the next visit", () => {
    localStorage.setItem(RECENT_VISITS_STORAGE_KEY, "{oops");
    render(<Reader />);
    expect(screen.getByTestId("out").textContent).toBe("");
    act(() => recordVisit(visit("z")));
    expect(screen.getByTestId("out").textContent).toBe("z");
  });

  it("clear empties the list and storage", () => {
    render(<Reader />);
    act(() => recordVisit(visit("a")));
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(screen.getByTestId("out").textContent).toBe("");
    expect(localStorage.getItem(RECENT_VISITS_STORAGE_KEY)).toBeNull();
  });

  it("falls back to memory when storage throws", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("QuotaExceeded");
    });
    render(<Reader />);
    act(() => recordVisit(visit("m")));
    expect(screen.getByTestId("out").textContent).toBe("m");
  });

  it("RecordVisit with null records nothing", () => {
    render(<RecordVisit visit={null} />);
    expect(localStorage.getItem(RECENT_VISITS_STORAGE_KEY)).toBeNull();
  });
});
