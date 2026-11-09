// @vitest-environment jsdom
/**
 * Cover morph naming (lib/ui/cover-morph.ts, components/library/CoverMorph).
 *
 * Regression: the dev log showed "There are two <ViewTransition
 * name=\"series-cover-…\"> components with the same name mounted at the
 * same time" on a series page. React forgets a name only when its
 * `<ViewTransition>` fiber unmounts, so a card that dropped its name while
 * still mounted (the in-flight mark clearing before a slow navigation
 * finished) left a stale entry, and the series header tripped over it.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { ViewTransition } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: () => {} }),
}));
vi.mock("next/link", () => ({
  default: ({
    href,
    onClick,
    children,
    ...rest
  }: React.AnchorHTMLAttributes<HTMLAnchorElement> & { href: string }) => (
    <a
      href={href}
      {...rest}
      onClick={(e) => {
        onClick?.(e);
        e.preventDefault();
      }}
    >
      {children}
    </a>
  ),
}));
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));
vi.mock("@/lib/api/mutations", () => ({
  useDismissRailItem: () => ({ mutate: () => {}, isPending: false }),
  useUpsertIssueProgress: () => ({ mutate: () => {}, isPending: false }),
}));
vi.mock("@/components/CoverLongPressActions", () => ({
  useCoverLongPressActions: () => ({ wrapperProps: {}, sheet: null }),
}));
vi.mock("@/components/collections/useCoverMenuCollectionActions", () => ({
  useCoverMenuCollectionActions: () => ({ actions: [], dialog: null }),
}));
vi.mock("@/components/CoverMenuButton", () => ({
  CoverMenuButton: () => null,
}));

import { CoverMorph } from "@/components/library/CoverMorph";
import { ProgressIssueCard } from "@/components/library/ProgressIssueCard";
import {
  ReaderCoverHandoff,
  ReaderCoverSlot,
} from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderCoverMorph";
import type { ContinueReadingCard } from "@/lib/api/types";
import {
  peekReaderHandoff,
  seriesCoverName,
  useActiveMorph,
  useCoverMorph,
} from "@/lib/ui/cover-morph";

const NAME = seriesCoverName("2099-omega");

function Card({ id }: { id: string }) {
  const morph = useCoverMorph(NAME, "/cover.jpg");
  return (
    <a
      href="/series/2099-omega"
      data-testid={id}
      onClick={(e) => {
        morph.onClick(e);
        e.preventDefault();
      }}
    >
      <span data-morph-name={morph.name ?? ""} />
      <CoverMorph name={morph.name}>
        <span />
      </CoverMorph>
    </a>
  );
}

function Hero() {
  return (
    <ViewTransition name={NAME} share="morph" default="none">
      <span />
    </ViewTransition>
  );
}

/** The reader route: loading fallback, then the reader. */
function ReaderRoute({ loaded }: { loaded: boolean }) {
  return loaded ? <ReaderCoverHandoff /> : <ReaderCoverSlot />;
}

function ActiveProbe() {
  const m = useActiveMorph();
  return <output data-testid="active">{m ? `${m.name}|${m.src}` : ""}</output>;
}

let errors: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  vi.useFakeTimers();
  errors = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  // Let any in-flight mark expire so tests don't leak into each other.
  act(() => vi.advanceTimersByTime(5000));
  vi.useRealTimers();
  errors.mockRestore();
});
const duplicateWarnings = () =>
  errors.mock.calls.filter((c: unknown[]) =>
    String(c[0]).includes("There are two <ViewTransition"),
  );

describe("cover morph names", () => {
  it("only the tapped card of a series shown twice carries the name", () => {
    render(
      <>
        <Card id="rail-a" />
        <Card id="rail-b" />
      </>,
    );
    fireEvent.click(screen.getByTestId("rail-b"));
    const names = Array.from(
      document.querySelectorAll("[data-morph-name]"),
    ).map((el) => el.getAttribute("data-morph-name"));
    expect(names).toEqual(["", NAME]);
    expect(duplicateWarnings()).toEqual([]);
  });

  it("a card whose mark clears while mounted leaves no stale name behind", () => {
    const cards = render(
      <>
        <Card id="rail-a" />
        <Card id="rail-b" />
      </>,
    );
    fireEvent.click(screen.getByTestId("rail-a"));
    // A slow navigation: the mark expires while the card is on screen.
    act(() => vi.advanceTimersByTime(3000));
    cards.unmount();
    // The destination's header cover always carries the name.
    render(<Hero />);
    expect(duplicateWarnings()).toEqual([]);
  });
});

describe("cover morph into the reader", () => {
  const card = {
    issue: {
      id: "i1",
      slug: "issue-2",
      series_id: "s1",
      series_slug: "invincible",
      series_name: "Invincible",
      title: "Eight Is Enough",
      number: "2",
      page_count: 22,
      state: "active",
      cover_url: "/issues/i1/pages/0/thumb",
    },
    series_name: "Invincible",
    progress: { last_page: 4, percent: 0.2, updated_at: "" },
  } as unknown as ContinueReadingCard;

  it("a Continue reading tap sends its cover into the reader", () => {
    render(
      <>
        <ProgressIssueCard card={card} primaryAction="reader" />
        <ActiveProbe />
      </>,
    );
    fireEvent.click(
      screen.getByRole("link", { name: /^Continue reading Eight Is Enough/ }),
    );
    expect(screen.getByTestId("active").textContent).toBe(
      "issue-cover-invincible-issue-2|/issues/i1/pages/0/thumb",
    );
  });

  it("the loading slot holds the cover past the mark and hands it over", () => {
    const tap = render(
      <ProgressIssueCard card={card} primaryAction="reader" />,
    );
    fireEvent.click(
      screen.getByRole("link", { name: /^Continue reading Eight Is Enough/ }),
    );
    tap.unmount();
    const route = render(<ReaderRoute loaded={false} />);
    expect(route.container.querySelector("img")?.getAttribute("src")).toBe(
      "/issues/i1/pages/0/thumb",
    );
    // The mark expires on a slow load; the slot keeps its cover.
    act(() => vi.advanceTimersByTime(3000));
    expect(route.container.querySelector("img")).not.toBeNull();
    expect(peekReaderHandoff()?.src).toBe("/issues/i1/pages/0/thumb");

    // The reader replaces the fallback in one commit, takes the cover
    // over under the same name, and the slot lets go of it.
    route.rerender(<ReaderRoute loaded />);
    expect(
      route.container.querySelector("[data-reader-cover-handoff] img"),
    ).not.toBeNull();
    expect(peekReaderHandoff()).toBeNull();
    expect(duplicateWarnings()).toEqual([]);
  });

  it("a cached reader route (no loading fallback) still takes the cover", () => {
    // Prefetched/cached reader data: Next skips loading.tsx, so the slot
    // never mounts and never holds a handoff. The reader must pick the
    // cover up from the morph in flight.
    const tap = render(
      <ProgressIssueCard card={card} primaryAction="reader" />,
    );
    fireEvent.click(
      screen.getByRole("link", { name: /^Continue reading Eight Is Enough/ }),
    );
    tap.unmount();
    expect(peekReaderHandoff()).toBeNull();
    const reader = render(<ReaderRoute loaded />);
    expect(
      reader.container
        .querySelector("[data-reader-cover-handoff] img")
        ?.getAttribute("src"),
    ).toBe("/issues/i1/pages/0/thumb");
  });

  it("a cold reader load shows the spinner and no handoff", () => {
    const slot = render(<ReaderCoverSlot />);
    expect(slot.container.querySelector("img")).toBeNull();
    const reader = render(<ReaderCoverHandoff />);
    expect(reader.container.innerHTML).toBe("");
  });

  // Regression: the cover sat over the reader for 4 s while the page turned
  // behind it, even when that page had already failed to load.
  describe("the cover stands in, never in the way", () => {
    const openReader = () => {
      const tap = render(
        <ProgressIssueCard card={card} primaryAction="reader" />,
      );
      fireEvent.click(
        screen.getByRole("link", { name: /^Continue reading Eight Is Enough/ }),
      );
      tap.unmount();
      return render(<ReaderRoute loaded />);
    };
    const covered = (r: { container: HTMLElement }) =>
      r.container.querySelector("[data-reader-cover-handoff]") !== null;
    /** A frame for the watcher, then the (zero-length, reduced motion) fade. */
    const settle = () => act(() => vi.advanceTimersByTime(40));

    it("gives way to a slow page on its own", () => {
      const reader = openReader();
      act(() => vi.advanceTimersByTime(1400));
      expect(covered(reader)).toBe(true);
      act(() => vi.advanceTimersByTime(200));
      settle();
      expect(covered(reader)).toBe(false);
    });

    it("lets go as soon as the page has failed", () => {
      const reader = openReader();
      settle();
      expect(covered(reader)).toBe(true);

      const failed = document.createElement("button");
      failed.setAttribute("data-page-load-error", "");
      document.body.append(failed);
      settle();
      settle();
      failed.remove();
      expect(covered(reader)).toBe(false);
    });

    it.each([
      ["a key", () => fireEvent.keyDown(window, { key: "ArrowRight" })],
      ["a tap", () => fireEvent.pointerDown(window)],
      ["the wheel", () => fireEvent.wheel(window)],
    ])("lets go on %s", (_name, input) => {
      const reader = openReader();
      settle();
      expect(covered(reader)).toBe(true);

      act(() => void input());
      settle();
      expect(covered(reader)).toBe(false);
    });
  });
});
