// @vitest-environment jsdom
/**
 * A scan row says when the run flagged files as findings (malformed,
 * encrypted and duplicate files each open one health finding) and links
 * to the library's findings; a clean run shows nothing.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  ScanRunFindingsLink,
  scanRunFlaggedFiles,
} from "@/components/admin/library/ScanRunFindings";

const HREF = "/admin/findings?tab=health&library_id=lib-1";

describe("scanRunFlaggedFiles", () => {
  it.each([
    [null, 0],
    [{}, 0],
    [{ files_added: 4 }, 0],
    [{ files_malformed: 2 }, 2],
    [{ files_malformed: 1, files_encrypted: 1, files_duplicate: 3 }, 5],
    [{ files_malformed: "2" }, 0],
  ])("%j → %d", (stats, n) => {
    expect(scanRunFlaggedFiles(stats)).toBe(n);
  });
});

describe("ScanRunFindingsLink", () => {
  it("links to the findings with the count when the run flagged files", () => {
    render(
      <ScanRunFindingsLink
        stats={{ files_malformed: 2, files_duplicate: 1 }}
        href={HREF}
      />,
    );
    const link = screen.getByRole("link", { name: "3 files flagged" });
    expect(link.getAttribute("href")).toBe(HREF);
  });

  it("uses the singular for one file", () => {
    render(<ScanRunFindingsLink stats={{ files_encrypted: 1 }} href={HREF} />);
    expect(screen.getByRole("link", { name: "1 file flagged" })).toBeTruthy();
  });

  it.each([[null], [{}], [{ files_added: 10, files_malformed: 0 }]])(
    "renders nothing for %j",
    (stats) => {
      const { container } = render(
        <ScanRunFindingsLink stats={stats} href={HREF} />,
      );
      expect(container.innerHTML).toBe("");
    },
  );
});
