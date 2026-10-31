// @vitest-environment jsdom
/**
 * Audit N-R10: the reader renders `contain` ("Screen" fit) as fit-width in
 * webtoon view. The settings popover must say so instead of offering an
 * option that silently does something else: in webtoon the Screen pill is
 * disabled and a hint explains why; in single / double view it is live and
 * the hint is absent.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
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
  WEBTOON_CONTAIN_HINT,
} from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderSettings";
import { useReaderStore } from "@/lib/reader/store";

function renderSettings(viewMode: "single" | "double" | "webtoon") {
  useReaderStore.setState({
    issueId: "issue-1",
    seriesId: "series-1",
    viewMode,
    fitMode: "contain",
    direction: "ltr",
  } as never);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ReaderSettings seriesId="series-1" />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
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

const disabledOf = (name: string) =>
  (screen.getByRole("button", { name }) as HTMLButtonElement).disabled;

describe("ReaderSettings fit options in webtoon view", () => {
  it("disables Screen fit and explains that webtoon fits to width", () => {
    renderSettings("webtoon");
    const screenFit = screen.getByRole("button", { name: "Screen" });
    expect((screenFit as HTMLButtonElement).disabled).toBe(true);
    expect(screenFit.getAttribute("title")).toBe(WEBTOON_CONTAIN_HINT);
    const hint = screen.getByText(WEBTOON_CONTAIN_HINT);
    // The hint is the group's description, so AT reads it with the control.
    const group = screen.getByRole("group", { name: "Fit mode" });
    expect(group.getAttribute("aria-describedby")).toBe(hint.id);
    // A stored `contain` renders as width in webtoon, and the control
    // shows that effective fit rather than a pressed-but-disabled Screen.
    expect(screenFit.getAttribute("aria-pressed")).toBe("false");
    expect(
      screen
        .getByRole("button", { name: "Width" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
    // The other fit modes still apply in webtoon and stay live.
    expect(disabledOf("Width")).toBe(false);
    expect(disabledOf("Height")).toBe(false);
  });

  it("keeps Screen fit live, without the hint, in single and double view", () => {
    for (const mode of ["single", "double"] as const) {
      const { unmount } = renderSettings(mode);
      const screenFit = screen.getByRole("button", { name: "Screen" });
      expect(disabledOf("Screen")).toBe(false);
      expect(screenFit.getAttribute("aria-pressed")).toBe("true");
      expect(screen.queryByText(WEBTOON_CONTAIN_HINT)).toBeNull();
      expect(
        screen
          .getByRole("group", { name: "Fit mode" })
          .hasAttribute("aria-describedby"),
      ).toBe(false);
      unmount();
    }
  });
});
