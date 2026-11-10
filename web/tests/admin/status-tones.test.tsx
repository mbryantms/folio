// @vitest-environment jsdom
/**
 * Admin status tones: ordinary states (Complete, Active, Idle, OK,
 * Cancelled) are neutral, work under way is info, and only states that
 * need the admin's attention take warning / error.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/scans",
  useSearchParams: () => new URLSearchParams(),
  useRouter: () => ({ replace: vi.fn() }),
}));

import { StatusBadge } from "@/components/admin/StatusBadge";
import {
  healthSeverityTone,
  jobStateTone,
  scanBatchStateTone,
  scanRunStateTone,
  serviceCheckTone,
  userStateTone,
} from "@/components/admin/status-tones";
import { StateBadge } from "@/components/admin/library/ScanRunsHistory";
import { statusTone } from "@/lib/ui/status-tone";

type Rule = (state: string) => string;
const TABLES: [string, Rule, [string, string][]][] = [
  [
    "scan run state",
    scanRunStateTone,
    [
      ["complete", "neutral"],
      ["completed", "neutral"],
      ["cancelled", "neutral"],
      ["running", "info"],
      ["queued", "info"],
      ["thumbnailing", "info"],
      ["failed", "error"],
    ],
  ],
  [
    "scan batch state",
    scanBatchStateTone,
    [
      ["complete", "neutral"],
      ["running", "info"],
      ["partial_failed", "warning"],
      ["failed", "error"],
    ],
  ],
  [
    "user state",
    userStateTone,
    [
      ["active", "neutral"],
      ["pending_verification", "warning"],
      ["disabled", "error"],
    ],
  ],
  [
    "job state",
    jobStateTone,
    [
      ["completed", "neutral"],
      ["cancelled", "neutral"],
      ["running", "info"],
      ["queued", "info"],
      ["awaiting_quota", "warning"],
      ["partial_failed", "warning"],
      ["failed", "error"],
    ],
  ],
  [
    "health severity",
    healthSeverityTone,
    [
      ["info", "neutral"],
      ["warning", "warning"],
      ["error", "error"],
    ],
  ],
];
const ROWS = TABLES.flatMap(([name, fn, rows]) =>
  rows.map(([state, tone]) => ({ name, fn, state, tone })),
);

describe("admin status tones", () => {
  it.each(ROWS)("$name: $state → $tone", ({ fn, state, tone }) => {
    expect(fn(state)).toBe(tone);
  });
});

describe("service check", () => {
  it("OK is neutral, down is error", () => {
    expect(serviceCheckTone(true)).toBe("neutral");
    expect(serviceCheckTone(false)).toBe("error");
  });
});

describe("StatusBadge", () => {
  it("draws the tone's token classes on the outline badge", () => {
    render(<StatusBadge tone="neutral">Active</StatusBadge>);
    const badge = screen.getByText("Active");
    expect(badge.getAttribute("data-tone")).toBe("neutral");
    expect(badge.getAttribute("data-variant")).toBe("outline");
    for (const cls of statusTone("neutral").split(" ")) {
      expect(badge.className.split(/\s+/)).toContain(cls);
    }
    // No brand-accent fill.
    expect(badge.className).not.toMatch(/\bbg-primary\b/);
  });

  it.each([
    ["complete", "neutral", "Complete"],
    ["running", "info", "Running"],
    ["failed", "error", "Failed"],
    ["cancelled", "neutral", "Cancelled"],
  ])("scan history %s badge is %s", (state, tone, label) => {
    render(<StateBadge state={state} />);
    const badge = screen.getByText(label).closest('[data-slot="badge"]')!;
    expect(badge.getAttribute("data-tone")).toBe(tone);
  });
});
