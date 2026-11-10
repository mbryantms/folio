// @vitest-environment jsdom
/**
 * Provider hints are for admins: a reader sees neither the series card's
 * "meta" chip (its fix is the admin-only match dialog) nor the entity
 * page's "Add a metadata provider under Admin" prompt; an admin sees both.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type * as Queries from "@/lib/api/queries";

import type { EntityDetailView, SeriesView } from "@/lib/api/types";

const me = vi.hoisted(() => ({ role: "user" as "user" | "admin" }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("@/lib/api/queries", async (orig) => ({
  ...(await orig<typeof Queries>()),
  useMe: () => ({ data: { id: "u1", role: me.role } }),
}));
vi.mock("@/components/library/use-cover-collection-dot", () => ({
  useCoverCollectionDot: () => ({ enabled: true, setEnabled: () => {} }),
}));

import { MetaNeedsBadge } from "@/components/library/SeriesCard";
import { EntityProfile } from "@/components/library/EntityProfile";

const sparseSeries = {
  slug: "doom",
  metadata_completeness_tier: "needs_metadata",
} as unknown as SeriesView;

const bareEntity = {
  kind: "characters",
  id: "c1",
  slug: "robin",
  name: "Robin",
  description: null,
  image_url: null,
  aliases: [],
  series_count: 3,
  issue_count: 12,
  links: [],
  enrichment: "unavailable",
} as unknown as EntityDetailView;

function renderBoth(isAdmin: boolean) {
  return render(
    <>
      <MetaNeedsBadge series={sparseSeries} interactive />
      <MetaNeedsBadge series={sparseSeries} interactive={false} />
      <EntityProfile kind="characters" detail={bareEntity} isAdmin={isAdmin} />
    </>,
  );
}

describe("provider hints", () => {
  it("a reader sees no meta chip and no admin prompt", () => {
    me.role = "user";
    const { container } = renderBoth(false);
    expect(screen.queryByText("meta")).toBeNull();
    expect(screen.queryByLabelText(/Meta likely incomplete/)).toBeNull();
    expect(screen.queryByLabelText("Metadata likely incomplete")).toBeNull();
    expect(container.textContent).not.toContain("Add a metadata provider");
    expect(container.textContent).not.toContain("Admin");
    expect(container.querySelector('a[href^="/admin"]')).toBeNull();
  });

  it("an admin sees the meta chip and the provider prompt", () => {
    me.role = "admin";
    const { container } = renderBoth(true);
    expect(screen.getAllByText("meta")).toHaveLength(2);
    expect(
      screen.getByRole("button", {
        name: "Meta likely incomplete — find metadata",
      }),
    ).toBeTruthy();
    expect(container.textContent).toContain(
      "Add a metadata provider under Admin → Metadata to fill this page.",
    );
  });
});
