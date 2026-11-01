// @vitest-environment jsdom
/**
 * F6 — the Vision section of the reader settings popover: every filter
 * slider's thumb is named (axe `aria-input-field-name`), the night-mode
 * switch is named, "Reset filters" appears only once something is off the
 * default and puts every filter back.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

vi.mock("next/link", () => ({
  default: ({ children, href }: { children: React.ReactNode; href: string }) =>
    React.createElement("a", { href }, children),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
  }),
}));

import {
  ReaderSettings,
  warmthLabel,
} from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderSettings";
import { DEFAULT_FILTERS } from "@/lib/reader/filters";
import { useReaderStore } from "@/lib/reader/store";

function renderSettings() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ReaderSettings seriesId="series-1" />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  localStorage.clear();
  useReaderStore.setState({
    issueId: "issue-1",
    seriesId: "series-1",
    viewMode: "single",
    fitMode: "width",
    direction: "ltr",
    ...DEFAULT_FILTERS,
  } as never);
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({
      ok: true,
      status: 200,
      headers: new Headers({ "content-type": "application/json" }),
      json: async () => ({}),
      text: async () => "{}",
    })),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ReaderSettings colour filters", () => {
  it("names every slider thumb and the night-mode switch", () => {
    renderSettings();
    for (const name of [
      "Brightness",
      "Contrast",
      "Grayscale",
      "Sepia",
      "Warmth",
    ]) {
      expect(screen.getByRole("slider", { name })).toBeTruthy();
    }
    expect(screen.getByRole("switch", { name: "Night mode" })).toBeTruthy();
    // Each thumb reports its value in the slider's own range.
    expect(
      screen
        .getByRole("slider", { name: "Contrast" })
        .getAttribute("aria-valuenow"),
    ).toBe("100");
    expect(
      screen
        .getByRole("slider", { name: "Warmth" })
        .getAttribute("aria-valuemin"),
    ).toBe("-50");
    // Nothing is off the default, so there is nothing to reset.
    expect(screen.queryByRole("button", { name: /reset filters/i })).toBeNull();
  });

  it("shows the value beside each label", () => {
    useReaderStore.setState({
      brightness: 1.2,
      contrast: 110,
      grayscale: 40,
      sepia: 0.35,
      warmth: -20,
    } as never);
    renderSettings();
    expect(screen.getByText("120%")).toBeTruthy();
    expect(screen.getByText("110%")).toBeTruthy();
    expect(screen.getByText("40%")).toBeTruthy();
    expect(screen.getByText("35%")).toBeTruthy();
    expect(screen.getByText(warmthLabel(-20))).toBeTruthy();
    expect(warmthLabel(0)).toBe("Neutral");
    expect(warmthLabel(25)).toBe("Warmer +25");
    expect(warmthLabel(-5)).toBe("Cooler −5");
  });

  it("night mode flips the store and surfaces Reset filters, which clears everything", () => {
    renderSettings();
    fireEvent.click(screen.getByRole("switch", { name: "Night mode" }));
    expect(useReaderStore.getState().invert).toBe(true);
    useReaderStore.setState({ grayscale: 60, warmth: 15 } as never);

    const reset = screen.getByRole("button", { name: /reset filters/i });
    fireEvent.click(reset);
    const s = useReaderStore.getState();
    expect(s.invert).toBe(false);
    expect(s.grayscale).toBe(0);
    expect(s.warmth).toBe(0);
    expect(s.brightness).toBe(1);
    expect(screen.queryByRole("button", { name: /reset filters/i })).toBeNull();
    expect(
      screen
        .getByRole("slider", { name: "Grayscale" })
        .getAttribute("aria-valuenow"),
    ).toBe("0");
  });
});
