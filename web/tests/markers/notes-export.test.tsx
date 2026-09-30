// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

import { copyMarkerLink } from "@/components/markers/MarkersList";
import { NotesExportMenu } from "@/components/markers/NotesExportMenu";
import { markerPermalink, notesExportHref } from "@/lib/urls";

describe("notes export + permalink URLs (WP-5.1)", () => {
  it("builds the export download href per format", () => {
    expect(notesExportHref("md")).toBe("/api/me/markers/export?format=md");
    expect(notesExportHref("json")).toBe("/api/me/markers/export?format=json");
  });

  it("builds a relative or absolute marker permalink", () => {
    const id = "00000000-0000-7000-8000-000000000001";
    expect(markerPermalink(id)).toBe(`/markers/${id}`);
    expect(markerPermalink(id, "https://folio.example")).toBe(
      `https://folio.example/markers/${id}`,
    );
  });
});

describe("NotesExportMenu", () => {
  it("offers Markdown and JSON as same-origin download links", () => {
    render(<NotesExportMenu />);
    const trigger = screen.getByRole("button", { name: /export notes/i });
    fireEvent.keyDown(trigger, { key: "Enter" });
    const md = screen.getByRole("menuitem", { name: /markdown/i });
    const json = screen.getByRole("menuitem", { name: /json/i });
    expect(md.getAttribute("href")).toBe(notesExportHref("md"));
    expect(md.hasAttribute("download")).toBe(true);
    expect(json.getAttribute("href")).toBe(notesExportHref("json"));
    expect(json.hasAttribute("download")).toBe(true);
  });
});

describe("copyMarkerLink", () => {
  it("writes the absolute permalink to the clipboard", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    await copyMarkerLink("abc");
    expect(writeText).toHaveBeenCalledWith(
      `${window.location.origin}/markers/abc`,
    );
  });
});
