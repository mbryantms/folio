// @vitest-environment jsdom
/**
 * Settings → Account email change: the form carries a current-password
 * field (required once the email changes), binds server field errors to it,
 * and shows the pending address, with a Cancel control, while a
 * confirmation link is outstanding. The real
 * `useMe` / `useUpdateAccount` hooks run against a mocked `apiFetch`.
 */
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

type Me = {
  id: string;
  role: string;
  display_name: string;
  email: string | null;
  email_editable: boolean;
  password_editable: boolean;
  pending_email: string | null;
};

const state = vi.hoisted(() => ({
  calls: [] as { path: string; method: string; body: unknown }[],
  me: null as unknown as Me,
  patch: null as null | ((body: Record<string, unknown>) => Response),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
}));

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => "csrf",
  apiFetch: async (path: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    state.calls.push({ path, method, body });
    if (path.startsWith("/auth/me")) return json(state.me);
    if (path === "/me/account" && method === "PATCH" && state.patch) {
      return state.patch(body);
    }
    return json({});
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/settings/account",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: state.toastSuccess,
    error: state.toastError,
    message: vi.fn(),
    info: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}));
// The other cards on the page fetch their own data; not under test here.
vi.mock("@/components/settings/SessionsCard", () => ({
  SessionsCard: () => null,
}));
vi.mock("@/components/settings/DataExportCard", () => ({
  DataExportCard: () => null,
}));
vi.mock("@/components/settings/HoldingsExportCard", () => ({
  HoldingsExportCard: () => null,
}));
vi.mock("@/components/settings/DeleteAccountCard", () => ({
  DeleteAccountCard: () => null,
}));

import {
  AccountForm,
  type EmailChangeStatus,
} from "@/components/settings/AccountForm";

function baseMe(over: Partial<Me> = {}): Me {
  return {
    id: "u1",
    role: "user",
    display_name: "Reader",
    email: "old@example.com",
    email_editable: true,
    password_editable: true,
    pending_email: null,
    ...over,
  };
}

function wrap(emailChange: EmailChangeStatus | null = null) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <AccountForm emailChange={emailChange} />
    </QueryClientProvider>,
  );
}

async function typeEmail(value: string) {
  const email = await screen.findByLabelText("Email");
  await act(async () => {
    fireEvent.change(email, { target: { value } });
  });
}

/** Queries scoped to the Profile form (the Change password card has its
 *  own "Current password" field). */
function profile() {
  const form = screen
    .getByRole("button", { name: "Save profile" })
    .closest("form");
  if (!form) throw new Error("profile form not found");
  return within(form);
}

function patches() {
  return state.calls.filter(
    (c) => c.path === "/me/account" && c.method === "PATCH",
  );
}

beforeEach(() => {
  state.calls.length = 0;
  state.me = baseMe();
  state.patch = null;
  state.toastSuccess.mockReset();
  state.toastError.mockReset();
});

describe("Account email change", () => {
  it("renders the current-password field up front for the form fallback", async () => {
    wrap();
    await screen.findByLabelText("Email");
    const field = profile().getByLabelText("Current password");
    expect(field.getAttribute("name")).toBe("current_password");
    expect(
      profile().getByText(/Only needed when you change your email/),
    ).toBeTruthy();
  });

  it("refuses to submit without the password", async () => {
    wrap();
    await typeEmail("new@example.com");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save profile" }));
    });
    expect(
      await screen.findByText(
        "Enter your current password to change your email",
      ),
    ).toBeTruthy();
    expect(patches()).toHaveLength(0);
  });

  it("sends the password and shows the pending address", async () => {
    state.patch = (body) => {
      state.me = baseMe({ pending_email: String(body.email) });
      return json(state.me);
    };
    wrap();
    await typeEmail("new@example.com");
    await act(async () => {
      fireEvent.change(profile().getByLabelText("Current password"), {
        target: { value: "hunter2-hunter2" },
      });
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save profile" }));
    });
    await waitFor(() =>
      expect(patches()[0]?.body).toEqual({
        email: "new@example.com",
        current_password: "hunter2-hunter2",
      }),
    );
    expect(
      await screen.findByText("Confirmation sent to new@example.com"),
    ).toBeTruthy();
    expect(state.toastSuccess).toHaveBeenCalledWith(
      "Confirmation sent to new@example.com",
      expect.anything(),
    );
  });

  it("binds a wrong-password error to the password field", async () => {
    state.patch = () =>
      json(
        {
          error: {
            code: "auth.invalid",
            message: "current password is incorrect",
            details: [
              {
                field: "current_password",
                message: "current password is incorrect",
              },
            ],
          },
        },
        403,
      );
    wrap();
    await typeEmail("new@example.com");
    await act(async () => {
      fireEvent.change(profile().getByLabelText("Current password"), {
        target: { value: "wrong-password" },
      });
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save profile" }));
    });
    expect(
      await screen.findByText("current password is incorrect"),
    ).toBeTruthy();
  });

  it("shows an outstanding change on load", async () => {
    state.me = baseMe({ pending_email: "pending@example.com" });
    wrap();
    expect(
      await screen.findByText("Confirmation sent to pending@example.com"),
    ).toBeTruthy();
  });

  it("does not ask accounts without a password for one", async () => {
    state.me = baseMe({ password_editable: false });
    wrap();
    await typeEmail("new@example.com");
    expect(profile().queryByLabelText("Current password")).toBeNull();
  });

  it("cancels a pending change", async () => {
    state.me = baseMe({ pending_email: "pending@example.com" });
    state.patch = (body) => {
      state.me = baseMe({ email: String(body.email), pending_email: null });
      return json(state.me);
    };
    wrap();
    await screen.findByText("Confirmation sent to pending@example.com");
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Cancel email change" }),
      );
    });
    await waitFor(() =>
      expect(patches()[0]?.body).toEqual({ email: "old@example.com" }),
    );
    await waitFor(() =>
      expect(
        screen.queryByText("Confirmation sent to pending@example.com"),
      ).toBeNull(),
    );
    expect(state.toastSuccess).toHaveBeenCalledWith(
      "Email change cancelled",
      expect.anything(),
    );
  });

  it("binds a wrong password on the password card to its field", async () => {
    state.patch = () =>
      json(
        {
          error: {
            code: "auth.invalid",
            message: "current password is incorrect",
            details: [
              {
                field: "current_password",
                message: "current password is incorrect",
              },
            ],
          },
        },
        403,
      );
    wrap();
    const button = await screen.findByRole("button", {
      name: "Change password",
    });
    const card = within(button.closest("form")!);
    await act(async () => {
      fireEvent.change(card.getByLabelText("Current password"), {
        target: { value: "wrong-password" },
      });
      fireEvent.change(card.getByLabelText("New password"), {
        target: { value: "a-whole-new-passphrase" },
      });
      fireEvent.change(card.getByLabelText("Confirm new password"), {
        target: { value: "a-whole-new-passphrase" },
      });
    });
    await act(async () => {
      fireEvent.click(button);
    });
    expect(await card.findByText("current password is incorrect")).toBeTruthy();
  });

  it("says every device was signed out when no session was kept", async () => {
    wrap("confirmed_signed_out");
    expect(
      await screen.findByText(/Every device signed in to that account/),
    ).toBeTruthy();
  });

  it("reports the outcome of a confirmation link", async () => {
    wrap("invalid");
    expect(
      await screen.findByText("That confirmation link didn’t work"),
    ).toBeTruthy();
  });
});
