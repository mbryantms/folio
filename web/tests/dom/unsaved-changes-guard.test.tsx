// @vitest-environment jsdom
/**
 * Audit 2026-11 U-18: leaving a dirty form asks in an `AlertDialog`
 * instead of `window.confirm`. The link only navigates on "Leave page";
 * Stay and Escape keep the form.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";

import { useUnsavedChangesGuard } from "@/lib/ui/use-unsaved-changes-guard";

function Harness({ dirty, onNav }: { dirty: boolean; onNav: () => void }) {
  const leaveGuard = useUnsavedChangesGuard(dirty);
  return (
    <>
      <a
        href="/elsewhere"
        onClick={(e) => {
          e.preventDefault();
          onNav();
        }}
      >
        Elsewhere
      </a>
      {leaveGuard}
    </>
  );
}

it("navigates straight away when the form is clean", () => {
  const onNav = vi.fn();
  render(<Harness dirty={false} onNav={onNav} />);
  fireEvent.click(screen.getByRole("link", { name: "Elsewhere" }));
  expect(onNav).toHaveBeenCalledOnce();
  expect(screen.queryByRole("alertdialog")).toBeNull();
});

it("asks first, and navigates only on Leave page", () => {
  const confirm = vi.spyOn(window, "confirm");
  const onNav = vi.fn();
  render(<Harness dirty onNav={onNav} />);
  fireEvent.click(screen.getByRole("link", { name: "Elsewhere" }));
  expect(onNav).not.toHaveBeenCalled();
  expect(
    screen.getByRole("alertdialog", { name: "Leave this page?" }),
  ).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Leave page" }));
  expect(onNav).toHaveBeenCalledOnce();
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(confirm).not.toHaveBeenCalled();
  confirm.mockRestore();
});

it("stays on Stay and on Escape", () => {
  const onNav = vi.fn();
  render(<Harness dirty onNav={onNav} />);
  const link = screen.getByRole("link", { name: "Elsewhere" });
  fireEvent.click(link);
  fireEvent.click(screen.getByRole("button", { name: "Stay" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  fireEvent.click(link);
  fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(onNav).not.toHaveBeenCalled();
});
