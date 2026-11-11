// @vitest-environment jsdom
/**
 * Audit 2026-11 U-18: the milestone celebration is a modal built on the
 * Radix dialog — it is a named, described dialog, traps focus, closes on
 * Escape and hands focus back to the Replay button that opened it.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";

vi.mock("@/lib/visual-effects", () => ({ useReducedMotion: () => false }));

import { Celebration, Milestones } from "@/components/log/overview/Milestones";
import type { Milestone } from "@/lib/log/overview";

const streak: Milestone = {
  id: "streak-7",
  icon: "flame",
  title: "7-day streak",
  sub: "Read every day for a week",
  earned: true,
};

it("is a dialog named by its title and described by its line", () => {
  render(
    <Celebration
      title={streak.title}
      sub={streak.sub}
      covers={[]}
      onClose={() => {}}
    />,
  );
  const dialog = screen.getByRole("dialog", { name: "7-day streak" });
  const describedBy = dialog.getAttribute("aria-describedby");
  expect(document.getElementById(describedBy!)?.textContent).toBe(
    "Read every day for a week",
  );
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: "Nice" }),
  );
});

it("closes on Escape", async () => {
  const onClose = vi.fn();
  render(
    <Celebration
      title={streak.title}
      sub={streak.sub}
      covers={[]}
      onClose={onClose}
    />,
  );
  fireEvent.keyDown(screen.getByRole("button", { name: "Nice" }), {
    key: "Escape",
  });
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
});

it("returns focus to the Replay button after Escape", async () => {
  render(<Milestones items={[streak]} coversFor={() => []} />);
  const replay = screen.getByRole("button", { name: "Replay 7-day streak" });
  replay.focus();
  fireEvent.click(replay);
  expect(screen.getByRole("dialog", { name: "7-day streak" })).toBeTruthy();
  fireEvent.keyDown(document.activeElement!, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(document.activeElement).toBe(replay);
});
