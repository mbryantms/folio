// @vitest-environment jsdom
/**
 * The series page's read-progress tiles read the series detail query
 * (seeded with the server-rendered series), so a progress write that
 * invalidates it updates them without a page reload.
 */
import { act, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";

import { SeriesProgressStats } from "@/app/[locale]/(library)/series/[slug]/SeriesProgressStats";
import { queryKeys } from "@/lib/api/queries";
import type { SeriesView } from "@/lib/api/types";

const series = {
  id: "s1",
  slug: "saga",
  name: "Saga",
  issue_count: 3,
  total_page_count: 60,
  progress_summary: { finished: 1, total: 3, finished_pages: 20 },
} as unknown as SeriesView;

describe("<SeriesProgressStats>", () => {
  it("renders the server copy, then follows the series query", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { staleTime: 30_000, retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <SeriesProgressStats series={series} />
      </QueryClientProvider>,
    );
    expect(screen.getByText("2 left")).toBeTruthy();
    await act(async () => {
      client.setQueryData<SeriesView>(queryKeys.series("saga"), {
        ...series,
        progress_summary: { finished: 2, total: 3, finished_pages: 40 },
      } as SeriesView);
    });
    expect(await screen.findByText("1 left")).toBeTruthy();
  });

  it("lets the server copy replace an older cached entry on arrival", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { staleTime: 30_000, retry: false } },
    });
    // Left by an earlier visit, before an issue was finished elsewhere.
    client.setQueryData<SeriesView>(queryKeys.series("saga"), {
      ...series,
      progress_summary: { finished: 0, total: 3, finished_pages: 0 },
    } as SeriesView);
    render(
      <QueryClientProvider client={client}>
        <SeriesProgressStats series={series} />
      </QueryClientProvider>,
    );
    expect(await screen.findByText("2 left")).toBeTruthy();
  });
});
