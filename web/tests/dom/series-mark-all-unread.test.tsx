// @vitest-environment jsdom
/**
 * Series-level "Mark all unread" resets every issue's read state and
 * saved page with no Undo, so it confirms with an AlertDialog like "Mark
 * all read" — on the series card (cover kebab / long-press sheet share
 * one action list) and in the series page's ⋯ menu. The real mutation
 * hooks run against a mocked `apiFetch`; the test asserts no progress
 * POST leaves before Confirm.
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
}));

function json(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => "csrf",
  apiFetch: async (path: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    state.calls.push({ path, method, body });
    if (path.startsWith("/auth/me")) return json({ id: "u1", role: "user" });
    if (path.endsWith("/progress") && method === "POST") {
      return json({ updated: 12 });
    }
    if (path.startsWith("/me/collections")) return json([]);
    return json({ items: [], next_cursor: null });
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/series/saga",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("next/link", () => ({
  default: ({
    href,
    children,
    ...rest
  }: React.AnchorHTMLAttributes<HTMLAnchorElement> & { href: string }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    message: vi.fn(),
    info: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}));
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));
vi.mock("@/components/collections/useCoverMenuCollectionActions", () => ({
  useCoverMenuCollectionActions: () => ({ actions: [], dialog: null }),
}));
// The kebab is a hover-only Radix menu; render its actions as plain
// buttons. The long-press sheet receives the same `menuActions` list.
vi.mock("@/components/CoverMenuButton", () => ({
  CoverMenuButton: ({
    actions,
  }: {
    actions: { label: string; onSelect: () => void }[];
  }) => (
    <div>
      {actions.map((a) => (
        <button key={a.label} type="button" onClick={a.onSelect}>
          {a.label}
        </button>
      ))}
    </div>
  ),
}));

import { SeriesCard } from "@/components/library/SeriesCard";
import { SeriesSettingsMenu } from "@/app/[locale]/(library)/series/[slug]/SeriesSettingsMenu";
import type { SeriesView } from "@/lib/api/types";

const series = {
  id: "s-1",
  slug: "saga",
  name: "Saga",
  year: 2012,
  issue_count: 12,
  cover_url: null,
  status: "continuing",
} as unknown as SeriesView;

function wrap(node: React.ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

const progressPosts = () =>
  state.calls.filter(
    (c) => c.path.endsWith("/progress") && c.method === "POST",
  );

beforeEach(() => {
  state.calls.length = 0;
});

async function confirmFlow(trigger: () => void) {
  trigger();
  const dialog = await screen.findByRole("alertdialog");
  expect(dialog.textContent).toMatch(/Mark every issue in Saga as unread\?/);
  // Nothing is written while the dialog is up.
  expect(progressPosts()).toEqual([]);

  // Cancel writes nothing.
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(progressPosts()).toEqual([]);

  trigger();
  await screen.findByRole("alertdialog");
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Mark unread" }));
  });
  await waitFor(() => expect(progressPosts()).toHaveLength(1));
  expect(progressPosts()[0]?.body).toEqual({ finished: false });
}

describe("series Mark all unread", () => {
  it("confirms on the series card before writing", async () => {
    wrap(<SeriesCard series={series} />);
    await confirmFlow(() =>
      fireEvent.click(screen.getByRole("button", { name: "Mark all unread" })),
    );
  });

  it("confirms in the series ⋯ menu before writing", async () => {
    wrap(
      <SeriesSettingsMenu
        seriesId="s-1"
        seriesSlug="saga"
        seriesName="Saga"
        libraryId="lib-1"
        firstIssue={null}
        readIncognitoHref={null}
      />,
    );
    await confirmFlow(() => {
      const trigger = screen.getByRole("button", { name: "Series actions" });
      // Radix opens on pointerdown in jsdom.
      fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
      fireEvent.click(
        screen.getByRole("menuitem", { name: "Mark all as unread" }),
      );
    });
  });
});
