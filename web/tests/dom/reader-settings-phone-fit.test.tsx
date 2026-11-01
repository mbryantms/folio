// @vitest-environment jsdom
/**
 * Audit-2026-11 C6: on a phone the reader settings popover's Fit control
 * (Width / Height / Original / Screen) ran off the popover's right edge,
 * Screen clipped. The View and Fit rows now stack the label above a
 * full-width segmented control whose segments share the row equally and
 * truncate rather than overflow; Direction stays beside its label. jsdom
 * has no layout, so this pins the classes and structure that produce it.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
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

import { ReaderSettings } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderSettings";
import { SegmentedControl } from "@/components/settings/SegmentedControl";
import { useReaderStore } from "@/lib/reader/store";

function renderSettings() {
  useReaderStore.setState({
    issueId: "issue-1",
    seriesId: "series-1",
    viewMode: "single",
    fitMode: "width",
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

const classesOf = (el: Element) => el.className.split(/\s+/);
/** The `Field` row holding a control: label span + control wrapper. */
const rowOf = (group: Element) => group.parentElement!.parentElement!;

describe("reader settings rows on a phone", () => {
  it("stacks the Fit label above a full-width control with equal segments", () => {
    renderSettings();
    const group = screen.getByRole("group", { name: "Fit mode" });
    const row = rowOf(group);
    expect(classesOf(row)).toEqual(
      expect.arrayContaining(["flex", "flex-col"]),
    );
    // Label first, control below it.
    expect(row.firstElementChild?.textContent).toBe("Fit");
    expect(row.lastElementChild?.contains(group)).toBe(true);
    expect(classesOf(row.lastElementChild!)).toContain("min-w-0");

    // The group spans the row and does not scroll (a scroll container
    // would clip the segments' focus rings).
    expect(classesOf(group)).toEqual(
      expect.arrayContaining(["flex", "w-full"]),
    );
    expect(classesOf(group)).not.toContain("inline-flex");
    expect(classesOf(group)).not.toContain("overflow-x-auto");

    const segments = within(group).getAllByRole("button");
    expect(segments.map((b) => b.textContent)).toEqual([
      "Width",
      "Height",
      "Original",
      "Screen",
    ]);
    for (const seg of segments) {
      expect(classesOf(seg)).toEqual(
        expect.arrayContaining(["flex-1", "min-w-0", "folio-tap"]),
      );
      // Truncation lives on an inner span: overflow-hidden on the button
      // would clip its touch tap area (`::after`).
      expect(classesOf(seg)).not.toContain("truncate");
      expect(classesOf(seg.firstElementChild!)).toContain("truncate");
    }
  });

  it("stacks View the same way and keeps Direction beside its label", () => {
    renderSettings();
    const view = screen.getByRole("group", { name: "View mode" });
    expect(classesOf(rowOf(view))).toContain("flex-col");
    expect(rowOf(view).firstElementChild?.textContent).toBe("View");
    expect(classesOf(view)).toContain("w-full");

    const direction = screen.getByRole("group", { name: "Reading direction" });
    const directionRow = rowOf(direction);
    expect(classesOf(directionRow)).not.toContain("flex-col");
    expect(classesOf(directionRow)).toEqual(
      expect.arrayContaining(["items-center", "justify-between"]),
    );
    expect(classesOf(direction)).toContain("inline-flex");
  });
});

describe("SegmentedControl", () => {
  const OPTIONS = [
    { value: "a", label: "First option" },
    { value: "b", label: "Second" },
    { value: "c", label: "Third" },
  ] as const;

  for (const fill of [false, true]) {
    it(`renders every option with its visible label as the name (fill=${fill})`, () => {
      render(
        <SegmentedControl
          value="b"
          onChange={() => {}}
          options={OPTIONS}
          ariaLabel="Pick one"
          fill={fill}
        />,
      );
      const group = screen.getByRole("group", { name: "Pick one" });
      const buttons = within(group).getAllByRole("button");
      expect(buttons).toHaveLength(OPTIONS.length);
      OPTIONS.forEach((opt, i) => {
        const button = buttons[i]!;
        // axe label-in-name: the accessible name is the visible text,
        // whole, even when the fill layout truncates it visually.
        expect(within(group).getByRole("button", { name: opt.label })).toBe(
          button,
        );
        expect(button.textContent).toBe(opt.label);
        expect(button.getAttribute("aria-pressed")).toBe(
          opt.value === "b" ? "true" : "false",
        );
      });
    });
  }

  it("keeps the default content-sized, scrollable layout", () => {
    render(
      <SegmentedControl
        value="a"
        onChange={() => {}}
        options={OPTIONS}
        ariaLabel="Pick one"
      />,
    );
    const group = screen.getByRole("group", { name: "Pick one" });
    expect(classesOf(group)).toEqual(
      expect.arrayContaining([
        "inline-flex",
        "max-w-full",
        "overflow-x-auto",
        "[&>*]:shrink-0",
      ]),
    );
    for (const b of within(group).getAllByRole("button")) {
      expect(classesOf(b)).toContain("px-3");
      expect(classesOf(b)).not.toContain("flex-1");
      expect(b.firstElementChild).toBeNull();
    }
  });
});
