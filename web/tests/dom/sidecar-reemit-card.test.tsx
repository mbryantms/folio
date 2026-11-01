// @vitest-environment jsdom
/**
 * Audit 2026-11 A7: "Rewrite metadata files" on the library Settings tab.
 * The button only works when both writeback flags are on, and the run only
 * starts after the confirm dialog that says every archive is rewritten.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  start: vi.fn(),
  library: {
    name: "Marvel",
    allow_archive_writeback: true,
    metadata_writeback_enabled: true,
  } as Record<string, unknown>,
  run: null as Record<string, unknown> | null,
}));

vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: unknown }) => (
    <a href={href}>{children as never}</a>
  ),
}));
vi.mock("@/lib/api/queries", () => ({
  useLibrary: () => ({ data: m.library }),
  useSidecarReemit: () => ({ data: { run: m.run } }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useStartSidecarReemit: () => ({ mutate: m.start, isPending: false }),
}));

import { SidecarReemitCard } from "@/components/admin/library/SidecarReemitCard";

beforeEach(() => {
  m.start.mockReset();
  m.library = {
    name: "Marvel",
    allow_archive_writeback: true,
    metadata_writeback_enabled: true,
  };
  m.run = null;
});

const button = () =>
  screen.getByRole("button", { name: "Rewrite metadata files" });

describe("<SidecarReemitCard>", () => {
  it("confirms before starting and says every archive is rewritten", async () => {
    render(<SidecarReemitCard slug="marvel" />);
    fireEvent.click(button());
    expect(m.start).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("rewrites every archive in Marvel");
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Rewrite all archives" }),
      );
    });
    expect(m.start).toHaveBeenCalledOnce();
  });

  it("cancel leaves the library alone", async () => {
    render(<SidecarReemitCard slug="marvel" />);
    fireEvent.click(button());
    await screen.findByRole("alertdialog");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    });
    expect(m.start).not.toHaveBeenCalled();
  });

  it.each([
    [false, true],
    [true, false],
  ])(
    "is disabled unless both writeback flags are on (archive %s, metadata %s)",
    (archive, metadata) => {
      m.library = {
        name: "Marvel",
        allow_archive_writeback: archive,
        metadata_writeback_enabled: metadata,
      };
      render(<SidecarReemitCard slug="marvel" />);
      expect((button() as HTMLButtonElement).disabled).toBe(true);
      expect(
        screen.getByText(/Needs both archive writeback and metadata writeback/),
      ).toBeTruthy();
    },
  );

  it("is disabled while a run is live and shows its counts", () => {
    m.run = {
      run_id: "r1",
      library_id: "a",
      status: "running",
      started_at: "2026-11-01T00:00:00Z",
      finished_at: null,
      total: 100,
      enqueued: 40,
      skipped_refused: 2,
      skipped_gone: 0,
      rewritten: 30,
      failed: 1,
      outstanding: 9,
      stalled: false,
      note: null,
    };
    render(<SidecarReemitCard slug="marvel" />);
    const live = screen.getByRole("button", { name: "Rewriting…" });
    expect((live as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("33 / 100")).toBeTruthy();
    expect(screen.getByText(/30 rewritten/)).toBeTruthy();
  });

  it("can start again when the live run has stalled", () => {
    m.run = {
      run_id: "r1",
      library_id: "a",
      status: "running",
      started_at: "2026-11-01T00:00:00Z",
      finished_at: null,
      total: 100,
      enqueued: 40,
      skipped_refused: 0,
      skipped_gone: 0,
      rewritten: 30,
      failed: 0,
      outstanding: 10,
      stalled: true,
      note: null,
    };
    render(<SidecarReemitCard slug="marvel" />);
    expect((button() as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByText(/Stalled/)).toBeTruthy();
  });
});
