// @vitest-environment jsdom
/**
 * `<SeriesHoverPreview>` — the series-card hover peek. Regression: list
 * endpoints never sent `progress_summary`, so the bar always read "0 / N".
 * The server now batches the viewer's summary onto every list row; these
 * checks pin how the preview renders it.
 */
import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import type { SeriesView } from "@/lib/api/types";
import { SeriesHoverPreview } from "@/components/library/CardHoverPreview";

function series(overrides: Partial<SeriesView> = {}): SeriesView {
  return {
    id: "s1",
    slug: "saga",
    name: "Saga",
    status: "continuing",
    publisher: "Image",
    issue_count: 3,
    cover_url: null,
    summary: null,
    genres: [],
    ...overrides,
  } as unknown as SeriesView;
}

const summary = (finished: number, total: number) => ({
  finished,
  total,
  in_progress: 0,
  finished_pages: 0,
});

describe("SeriesHoverPreview progress", () => {
  it("shows the viewer's finished count from progress_summary", async () => {
    render(
      <SeriesHoverPreview
        series={series({ progress_summary: summary(2, 3) })}
      />,
    );
    await waitFor(() => expect(screen.getByText("2 / 3 read")).toBeTruthy());
    const bar = screen.getByRole("progressbar", {
      name: "Read 2 of 3 issues",
    });
    // The shadcn `<Progress>` paints `value` as the indicator's offset.
    const indicator = bar.firstElementChild as HTMLElement;
    expect(indicator.style.transform).toBe("translateX(-33%)");
  });

  it("says caught up when every issue is finished", async () => {
    render(
      <SeriesHoverPreview
        series={series({ progress_summary: summary(3, 3) })}
      />,
    );
    await waitFor(() => expect(screen.getByText("Caught up")).toBeTruthy());
  });

  it("renders no bar (and no NaN) for an empty series", async () => {
    const { container } = render(
      <SeriesHoverPreview
        series={series({ issue_count: 0, progress_summary: summary(0, 0) })}
      />,
    );
    await waitFor(() => expect(screen.getByText("Saga")).toBeTruthy());
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(container.textContent).not.toContain("NaN");
  });

  it("hides the bar instead of claiming 0 read when the summary is absent", async () => {
    render(<SeriesHoverPreview series={series()} />);
    await waitFor(() => expect(screen.getByText("Saga")).toBeTruthy());
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByText(/read$/)).toBeNull();
  });
});
