// @vitest-environment jsdom
/**
 * Remediation B9: the account toggle ("Count annuals and specials toward
 * series completion") on Settings → Reading, and the per-series
 * inherit / include / exclude control in the series ⋯ menu. Both go
 * through `useApiMutation` (success toast on the hook) and invalidate the
 * series views. The real hooks run against a mocked `apiFetch`.
 */
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  calls: [] as { path: string; method: string; body: unknown }[],
  countSpecials: false,
  refresh: vi.fn(),
  toastSuccess: vi.fn(),
}));

function json(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

function me() {
  return {
    id: "u1",
    role: "user",
    count_specials_in_completeness: state.countSpecials,
  };
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => "csrf",
  apiFetch: async (path: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    state.calls.push({ path, method, body });
    if (path.startsWith("/auth/me")) return json(me());
    if (path === "/me/preferences" && method === "PATCH") {
      state.countSpecials = body.count_specials_in_completeness;
      return json(me());
    }
    if (path.includes("/completion-prefs") && method === "PUT") {
      return json({
        count_specials: body.count_specials,
        effective: body.count_specials === "include",
      });
    }
    if (path.startsWith("/me/collections")) return json([]);
    return json({ items: [], next_cursor: null });
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({
    push: vi.fn(),
    replace: vi.fn(),
    refresh: state.refresh,
  }),
  usePathname: () => "/series/saga",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: state.toastSuccess,
    error: vi.fn(),
    message: vi.fn(),
    info: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}));

import { SeriesCompletionPref } from "@/components/settings/SeriesCompletionPref";
import { SeriesSettingsMenu } from "@/app/[locale]/(library)/series/[slug]/SeriesSettingsMenu";

function wrap(node: React.ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
  return { client, invalidate };
}

function invalidatedPrefixes(spy: { mock: { calls: unknown[][] } }): string[] {
  return spy.mock.calls.map((c) =>
    JSON.stringify((c[0] as { queryKey: unknown }).queryKey),
  );
}

beforeEach(() => {
  state.calls.length = 0;
  state.countSpecials = false;
  state.refresh.mockReset();
  state.toastSuccess.mockReset();
});

describe("Settings toggle", () => {
  it("PATCHes the account setting, toasts, and invalidates series views", async () => {
    const { invalidate } = wrap(<SeriesCompletionPref />);
    const toggle = await screen.findByRole("switch", {
      name: "Count annuals and specials toward series completion",
    });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(toggle.getAttribute("aria-describedby")).toBe("count-specials-help");

    await act(async () => {
      fireEvent.click(toggle);
    });

    await waitFor(() =>
      expect(
        state.calls.find(
          (c) => c.path === "/me/preferences" && c.method === "PATCH",
        )?.body,
      ).toEqual({ count_specials_in_completeness: true }),
    );
    await waitFor(() =>
      expect(state.toastSuccess).toHaveBeenCalledWith(
        "Annuals and specials now count toward completion",
        expect.anything(),
      ),
    );
    const keys = invalidatedPrefixes(invalidate);
    expect(keys).toEqual(
      expect.arrayContaining([
        '["series"]',
        '["saved-views"]',
        '["rails"]',
        '["collections"]',
      ]),
    );
    await waitFor(() =>
      expect(toggle.getAttribute("aria-checked")).toBe("true"),
    );
  });
});

function openMenu() {
  const trigger = screen.getByRole("button", { name: "Series actions" });
  // Radix opens on pointerdown in jsdom.
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
}

describe("Series ⋯ menu control", () => {
  function renderMenu(mode: "inherit" | "include" | "exclude") {
    return wrap(
      <SeriesSettingsMenu
        seriesId="s-1"
        seriesSlug="saga"
        seriesName="Saga"
        libraryId="lib-1"
        firstIssue={null}
        readIncognitoHref={null}
        specialsCompletion={mode}
      />,
    );
  }

  it("shows the current choice and PUTs a new one", async () => {
    const { invalidate } = renderMenu("inherit");
    openMenu();
    // Hover devices get a side submenu; open it.
    const sub = await screen.findByRole("menuitem", {
      name: /Specials in completion/,
    });
    fireEvent.pointerMove(sub);
    fireEvent.keyDown(sub, { key: "ArrowRight" });

    const inherit = await screen.findByRole("menuitemradio", {
      name: "Use my setting (main run only)",
    });
    expect(inherit.getAttribute("aria-checked")).toBe("true");
    const include = screen.getByRole("menuitemradio", {
      name: "Count annuals and specials",
    });
    expect(include.getAttribute("aria-checked")).toBe("false");

    await act(async () => {
      fireEvent.click(include);
    });

    await waitFor(() =>
      expect(
        state.calls.find((c) => c.path.includes("/completion-prefs")),
      ).toEqual({
        path: "/me/series/s-1/completion-prefs",
        method: "PUT",
        body: { count_specials: "include" },
      }),
    );
    await waitFor(() =>
      expect(state.toastSuccess).toHaveBeenCalledWith(
        "Specials count toward this series' completion",
        expect.anything(),
      ),
    );
    expect(state.refresh).toHaveBeenCalled();
    expect(invalidatedPrefixes(invalidate)).toContain('["series"]');
  });

  it("does not write when the current choice is picked again", async () => {
    renderMenu("exclude");
    openMenu();
    const sub = await screen.findByRole("menuitem", {
      name: /Specials in completion/,
    });
    fireEvent.keyDown(sub, { key: "ArrowRight" });
    const exclude = await screen.findByRole("menuitemradio", {
      name: "Main run only",
    });
    expect(exclude.getAttribute("aria-checked")).toBe("true");
    await act(async () => {
      fireEvent.click(exclude);
    });
    expect(state.calls.some((c) => c.path.includes("/completion-prefs"))).toBe(
      false,
    );
  });
});
