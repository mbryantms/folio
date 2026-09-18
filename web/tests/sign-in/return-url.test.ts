import { expect, it, vi } from "vitest";
const request = vi.hoisted(() => ({ headers: new Headers() }));
vi.mock("next/headers", () => ({ headers: async () => request.headers }));
import { signInUrl } from "@/lib/api/sign-in-url";
it("preserves a local destination including filters through sign-in", async () => {
  request.headers.set("x-folio-return-to", "/bookmarks?sort=newest");
  expect(await signInUrl()).toBe("/sign-in?next=%2Fbookmarks%3Fsort%3Dnewest");
});
it("rejects an external return target", async () => {
  request.headers.set("x-folio-return-to", "//evil.test");
  expect(await signInUrl()).toBe("/sign-in");
});
