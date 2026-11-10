/**
 * Server loaders encode route params into API paths: a library slug
 * holding `/`, `?`, `#` or `%` asks for that one library, not another path.
 */
import { afterEach, expect, it, vi } from "vitest";

vi.mock("@/components/admin/library/LibraryDangerZone", () => ({
  LibraryDangerZone: () => null,
}));
vi.mock("@/components/admin/library/LibrarySettingsForm", () => ({
  LibrarySettingsForm: () => null,
}));
vi.mock("@/components/admin/library/SidecarReemitCard", () => ({
  SidecarReemitCard: () => null,
}));
vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("NEXT_NOT_FOUND");
  },
}));

const apiGet = vi.fn();
vi.mock("@/lib/api/fetch", () => ({
  apiGet: (path: string) => apiGet(path),
  ApiError: class extends Error {},
}));

afterEach(() => {
  apiGet.mockReset();
});

it("requests the encoded library path", async () => {
  apiGet.mockResolvedValue({ name: "Comics" });
  const { default: Page } =
    await import("@/app/[locale]/(admin)/admin/libraries/[slug]/settings/page");
  await Page({ params: Promise.resolve({ slug: "a/b?c#d%" }) });
  expect(apiGet).toHaveBeenCalledOnce();
  expect(apiGet).toHaveBeenCalledWith("/libraries/a%2Fb%3Fc%23d%25");
});
