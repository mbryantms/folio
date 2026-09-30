// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  DataExportCard,
  EXPORT_HREF,
} from "@/components/settings/DataExportCard";

describe("DataExportCard", () => {
  it("renders a same-origin download link to the export endpoint", () => {
    render(<DataExportCard />);
    const link = screen.getByRole("link", { name: /export my data/i });
    // A plain anchor (not a fetch) so the session cookie rides along and
    // the browser honours the server's Content-Disposition filename.
    expect(link.getAttribute("href")).toBe(EXPORT_HREF);
    expect(link.hasAttribute("download")).toBe(true);
    expect(EXPORT_HREF).toBe("/api/me/export");
  });

  it("says what the file contains", () => {
    const { container } = render(<DataExportCard />);
    const text = container.textContent ?? "";
    expect(text).toMatch(/reading progress/i);
    expect(text).toMatch(/content hash/i);
  });
});
