// @vitest-environment jsdom
/**
 * Audit 2026-11 F5: Settings → Account "Delete account". The dialog points
 * at the export first, asks local accounts for their password and
 * identity-provider accounts for their email, and on success clears the
 * device and lands on the sign-in page's "account deleted" notice.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  mutate: vi.fn(),
  assign: vi.fn(),
  broadcast: vi.fn(),
  clearPrivate: vi.fn(async () => undefined),
  outboxClear: vi.fn(async () => undefined),
  downloadsClear: vi.fn(async () => undefined),
}));

vi.mock("@/lib/api/mutations", () => ({
  useDeleteAccount: () => ({ mutate: m.mutate, isPending: false }),
}));
vi.mock("@/lib/pwa/private-state", () => ({
  broadcastPrivateReset: m.broadcast,
  clearPrivateState: m.clearPrivate,
}));
vi.mock("@/lib/pwa/outbox", () => ({
  getOutbox: () => ({ clear: m.outboxClear }),
}));
vi.mock("@/lib/pwa/downloads", () => ({
  getDownloadManager: () => ({ clearAll: m.downloadsClear }),
}));

import {
  DELETED_HREF,
  DeleteAccountCard,
} from "@/components/settings/DeleteAccountCard";
import { EXPORT_HREF } from "@/components/settings/DataExportCard";
import { ApiMutationError } from "@/lib/api/mutations/_core";

function wrap(node: ReactNode) {
  const qc = new QueryClient();
  return render(<QueryClientProvider client={qc}>{node}</QueryClientProvider>);
}

const LOCAL = {
  email: "reader@example.com",
  display_name: "Reader",
  password_editable: true,
};
const OIDC = { ...LOCAL, password_editable: false };

beforeEach(() => {
  for (const f of Object.values(m)) f.mockReset();
  m.clearPrivate.mockImplementation(async () => undefined);
  m.outboxClear.mockImplementation(async () => undefined);
  m.downloadsClear.mockImplementation(async () => undefined);
  Object.defineProperty(window, "location", {
    configurable: true,
    value: { ...window.location, assign: m.assign },
  });
});

function openDialog() {
  fireEvent.click(screen.getByRole("button", { name: /delete account/i }));
  return screen.getByRole("alertdialog");
}

describe("<DeleteAccountCard>", () => {
  it("recommends the export and needs the password before deleting", async () => {
    wrap(<DeleteAccountCard me={LOCAL} />);
    const dialog = openDialog();

    const exportLink = screen.getByRole("link", { name: /export my data/i });
    expect(exportLink.getAttribute("href")).toBe(EXPORT_HREF);
    expect(dialog.textContent).toMatch(/download your data first/i);

    const submit = screen.getByRole("button", { name: /^delete account$/i });
    expect(submit.hasAttribute("disabled")).toBe(true);

    const input = screen.getByLabelText(/your password/i);
    expect(input.getAttribute("type")).toBe("password");
    fireEvent.change(input, { target: { value: "hunter2-hunter2" } });
    expect(submit.hasAttribute("disabled")).toBe(false);

    m.mutate.mockImplementation(
      (_body: unknown, opts: { onSuccess: () => void }) => opts.onSuccess(),
    );
    fireEvent.click(submit);
    expect(m.mutate).toHaveBeenCalledWith(
      { password: "hunter2-hunter2" },
      expect.anything(),
    );
    await waitFor(() => expect(m.assign).toHaveBeenCalledWith(DELETED_HREF));
    expect(m.broadcast).toHaveBeenCalled();
    expect(m.clearPrivate).toHaveBeenCalled();
    expect(m.outboxClear).toHaveBeenCalled();
    expect(m.downloadsClear).toHaveBeenCalled();
    // The form stays locked while the device cleanup runs: a second submit
    // would go out without the cookies the 204 cleared.
    expect(
      screen
        .getByRole("button", { name: /deleting/i })
        .hasAttribute("disabled"),
    ).toBe(true);
  });

  it("shows a wrong password inline and stays open", () => {
    wrap(<DeleteAccountCard me={LOCAL} />);
    openDialog();
    fireEvent.change(screen.getByLabelText(/your password/i), {
      target: { value: "wrong" },
    });
    m.mutate.mockImplementation(
      (_body: unknown, opts: { onError: (e: unknown) => void }) =>
        opts.onError(
          new ApiMutationError(
            "password is incorrect",
            403,
            [],
            "auth.invalid",
          ),
        ),
    );
    fireEvent.click(screen.getByRole("button", { name: /^delete account$/i }));
    expect(screen.getByRole("alertdialog").textContent).toMatch(
      /password is incorrect/i,
    );
    expect(m.assign).not.toHaveBeenCalled();
  });

  it("asks an identity-provider account to type its email", () => {
    wrap(<DeleteAccountCard me={OIDC} />);
    openDialog();
    const input = screen.getByLabelText(/type reader@example\.com to confirm/i);
    expect(input.getAttribute("type")).toBe("text");
    const submit = screen.getByRole("button", { name: /^delete account$/i });

    fireEvent.change(input, { target: { value: "someone@else.com" } });
    expect(submit.hasAttribute("disabled")).toBe(true);

    fireEvent.change(input, { target: { value: "Reader@Example.com" } });
    expect(submit.hasAttribute("disabled")).toBe(false);
    fireEvent.click(submit);
    expect(m.mutate).toHaveBeenCalledWith(
      { confirm: "Reader@Example.com" },
      expect.anything(),
    );
  });
});
