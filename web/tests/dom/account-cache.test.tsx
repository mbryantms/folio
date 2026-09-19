// @vitest-environment jsdom
import { act, render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
const reset = vi.hoisted(() => ({ clear: vi.fn(), broadcast: vi.fn() }));
vi.mock("@/lib/pwa/private-state", () => ({
  ACCOUNT_CHANNEL: "folio-account-test",
  clearPrivateState: reset.clear,
  broadcastPrivateReset: reset.broadcast,
}));
import { AccountCacheBoundary } from "@/components/AccountCacheBoundary";
import { queryKeys } from "@/lib/api/queries";
beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  // Keep the reset barrier pending: these tests assert invalidation before
  // navigation, without asking jsdom to implement a full document reload.
  reset.clear.mockImplementation(() => new Promise<void>(() => {}));
});
it("clears and broadcasts an observed identity change before reloading", async () => {
  const client = new QueryClient();
  client.setQueryData(queryKeys.me, { id: "a" });
  render(
    <QueryClientProvider client={client}>
      <AccountCacheBoundary userId="a" />
    </QueryClientProvider>,
  );
  expect(reset.clear).not.toHaveBeenCalled();
  await act(async () => {
    client.setQueryData(queryKeys.me, { id: "b" });
  });
  expect(reset.clear).toHaveBeenCalledWith(client);
  expect(reset.broadcast).toHaveBeenCalledOnce();
  expect(localStorage.getItem("folio:account-id")).toBe("b");
});
it("recognizes an account change while this tab was closed", () => {
  localStorage.setItem("folio:account-id", "a");
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <AccountCacheBoundary userId="b" />
    </QueryClientProvider>,
  );
  expect(reset.clear).toHaveBeenCalledWith(client);
  expect(reset.broadcast).toHaveBeenCalledOnce();
});
it("does not reset when signing in from an anonymous session", () => {
  // The sign-in page stamps "anonymous"; landing on the library afterwards
  // is the first identity, not a switch. A reset here reloads the landing
  // page on every sign-in (and raced the e2e reader-flow navigation).
  localStorage.setItem("folio:account-id", "anonymous");
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <AccountCacheBoundary userId="a" />
    </QueryClientProvider>,
  );
  expect(reset.clear).not.toHaveBeenCalled();
  expect(reset.broadcast).not.toHaveBeenCalled();
  expect(localStorage.getItem("folio:account-id")).toBe("a");
});
