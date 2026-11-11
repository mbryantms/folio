// @vitest-environment jsdom
/**
 * First-run setup card on the system Home. A fresh install auto-pins
 * system views onto Home, so the pinned list is never empty and the
 * empty rails hide themselves: the "create a library" card has to sit
 * above the rails whatever is pinned, for an admin with no library.
 */
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  role: "admin" as "admin" | "user",
  libraries: [] as Array<{ id: string }>,
}));

vi.mock("@/lib/api/queries", () => ({
  // Auto-pinned system views, as the server seeds them on first load.
  useSavedViews: () => ({
    data: {
      items: [
        { id: "sys-continue", kind: "system" },
        { id: "sys-on-deck", kind: "system" },
      ],
    },
    isLoading: false,
  }),
  useLibraryList: () => ({ data: state.libraries }),
  useMe: () => ({ data: { role: state.role } }),
}));
vi.mock("@/components/saved-views/PageActionsMenu", () => ({
  PageActionsMenu: () => null,
}));
vi.mock("@/components/saved-views/PageHeading", () => ({
  PageHeading: ({ pageName }: { pageName: string }) => <h1>{pageName}</h1>,
}));
// Empty system rails render nothing (SavedViewRail hides them).
vi.mock("@/components/saved-views/LazyRail", () => ({
  LazyRail: () => null,
}));

const { PageRails } = await import("@/components/saved-views/PageRails");

function renderHome() {
  return render(
    <PageRails
      pageId="sys-home"
      pageName="Home"
      pageDescription={null}
      isSystem
      showInSidebar
    />,
  );
}

describe("first-run setup card", () => {
  beforeEach(() => {
    state.role = "admin";
    state.libraries = [];
  });

  it("shows an admin with no library the setup card above auto-pinned rails", () => {
    renderHome();
    expect(
      screen.getByRole("heading", { name: "Set up your first library" }),
    ).toBeTruthy();
    const link = screen.getByRole("link", { name: "Create a library" });
    expect(link.getAttribute("href")).toBe("/admin/libraries");
  });

  it("is gone once a library exists", () => {
    state.libraries = [{ id: "lib-1" }];
    renderHome();
    expect(
      screen.queryByRole("heading", { name: "Set up your first library" }),
    ).toBeNull();
    expect(screen.queryByRole("link", { name: "Create a library" })).toBeNull();
  });

  it("tells a reader with no library that an admin has to act, with no admin link", () => {
    state.role = "user";
    renderHome();
    expect(
      screen.getByRole("heading", { name: "Nothing to read yet" }),
    ).toBeTruthy();
    expect(screen.queryByRole("link", { name: "Create a library" })).toBeNull();
  });

  it("stays off custom pages", () => {
    render(
      <PageRails
        pageId="page-marvel"
        pageName="Marvel"
        pageDescription={null}
        isSystem={false}
        showInSidebar
      />,
    );
    expect(screen.queryByRole("link", { name: "Create a library" })).toBeNull();
  });
});
