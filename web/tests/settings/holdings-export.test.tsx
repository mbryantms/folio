// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const useLibraryList = vi.fn();
vi.mock("@/lib/api/queries", () => ({
  useLibraryList: () => useLibraryList(),
}));

import {
  HoldingsExportCard,
  holdingsHref,
} from "@/components/settings/HoldingsExportCard";

beforeEach(() => {
  useLibraryList.mockReturnValue({
    data: [
      { id: "1", slug: "comics", name: "Comics" },
      { id: "2", slug: "manga", name: "Manga" },
    ],
  });
});

function link(): HTMLAnchorElement {
  return screen.getByRole("link", {
    name: /download holdings/i,
  }) as HTMLAnchorElement;
}

describe("HoldingsExportCard", () => {
  it("defaults to a series export of every library as a plain download link", () => {
    render(<HoldingsExportCard />);
    // A plain anchor (not fetch-to-blob) so a large CSV streams to disk and
    // the session cookie rides along.
    expect(link().getAttribute("href")).toBe(
      "/api/me/holdings.csv?scope=series",
    );
    expect(link().hasAttribute("download")).toBe(true);
  });

  it("follows the scope and library choices", () => {
    render(<HoldingsExportCard />);
    fireEvent.change(screen.getByRole("combobox", { name: "One row per" }), {
      target: { value: "issues" },
    });
    expect(link().getAttribute("href")).toBe(
      "/api/me/holdings.csv?scope=issues",
    );
    fireEvent.change(screen.getByRole("combobox", { name: "Library" }), {
      target: { value: "manga" },
    });
    expect(link().getAttribute("href")).toBe(
      "/api/me/holdings.csv?scope=issues&library=manga",
    );
    fireEvent.change(screen.getByRole("combobox", { name: "Library" }), {
      target: { value: "" },
    });
    expect(link().getAttribute("href")).toBe(
      "/api/me/holdings.csv?scope=issues",
    );
  });

  it("lists the caller's libraries after an all-libraries option", () => {
    render(<HoldingsExportCard />);
    const options = Array.from(
      screen
        .getByRole("combobox", { name: "Library" })
        .querySelectorAll("option"),
    ).map((o) => o.textContent);
    expect(options).toEqual(["All libraries", "Comics", "Manga"]);
  });

  it("still offers the all-libraries download while libraries load", () => {
    useLibraryList.mockReturnValue({ data: undefined });
    render(<HoldingsExportCard />);
    expect(link().getAttribute("href")).toBe(
      "/api/me/holdings.csv?scope=series",
    );
  });

  it("encodes the library slug", () => {
    expect(holdingsHref("series", "a b&c")).toBe(
      "/api/me/holdings.csv?scope=series&library=a+b%26c",
    );
  });
});
