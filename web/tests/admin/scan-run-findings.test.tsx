// @vitest-environment jsdom
/**
 * A scan row shows the run's exact open-finding count
 * (`health_issue_count`) and links to the findings page filtered to that
 * run; a run without findings shows nothing.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  ScanRunFindingsLink,
  scanRunFindingsHref,
} from "@/components/admin/library/ScanRunFindings";

const SCAN = "0190a4f2-0000-7000-8000-00000000abcd";

describe("ScanRunFindingsLink", () => {
  it("shows the exact count and links to the run's findings", () => {
    render(<ScanRunFindingsLink scanId={SCAN} count={7} />);
    const link = screen.getByRole("link", { name: "7 findings" });
    expect(link.getAttribute("href")).toBe(
      `/admin/findings?tab=health&scan_id=${SCAN}`,
    );
  });

  it("uses the singular for one finding", () => {
    render(<ScanRunFindingsLink scanId={SCAN} count={1} />);
    expect(screen.getByRole("link", { name: "1 finding" })).toBeTruthy();
  });

  it.each([[0], [-1], [Number.NaN]])("renders nothing for %d", (count) => {
    const { container } = render(
      <ScanRunFindingsLink scanId={SCAN} count={count} />,
    );
    expect(container.innerHTML).toBe("");
  });

  it("encodes the scan id into the query", () => {
    expect(scanRunFindingsHref("a b")).toBe(
      "/admin/findings?tab=health&scan_id=a%20b",
    );
  });
});
