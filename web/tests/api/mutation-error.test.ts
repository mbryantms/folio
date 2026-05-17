/**
 * Contracts for `ApiMutationError` and `apiMutate`'s error path.
 *
 * Anchored here so a refactor of `web/lib/api/mutations.ts` can't
 * silently degrade the toast UX:
 *
 *  - `.transient === true` for 5xx + network errors so the Retry
 *    action attaches on the error toast.
 *  - `.transient === false` for 4xx so we don't tempt the user
 *    into retrying a validation / auth / permission failure.
 *  - Structured server errors (`{ error: { message } }`) get
 *    unwrapped — otherwise toasts read `"[object Object]"`.
 *  - Network rejections from `fetch` (offline / DNS / TLS) become
 *    `status: "network"` rather than swallowing the cause.
 *
 * Tests live here in `tests/api/` alongside the other mutation
 * smoke checks. They don't render React — `ApiMutationError` is a
 * pure class and `apiMutate` is exercisable with a mocked
 * `apiFetch`.
 *
 * Cleanup plan: notifications-cleanup-1.0 post-ship #8.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock `apiFetch` so we can control the response without running a
// real network call. The mock is set per-test below.
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(),
}));

import { ApiMutationError, apiMutate } from "@/lib/api/mutations";
import { apiFetch } from "@/lib/api/auth-refresh";

const mockedFetch = vi.mocked(apiFetch);

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

beforeEach(() => {
  mockedFetch.mockReset();
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("ApiMutationError.transient", () => {
  it("treats network errors as transient", () => {
    const e = new ApiMutationError("Failed to fetch", "network");
    expect(e.transient).toBe(true);
  });

  it("treats 5xx as transient", () => {
    expect(new ApiMutationError("Internal Server Error", 500).transient).toBe(
      true,
    );
    expect(new ApiMutationError("Bad Gateway", 502).transient).toBe(true);
    expect(new ApiMutationError("Service Unavailable", 503).transient).toBe(
      true,
    );
    expect(new ApiMutationError("Gateway Timeout", 504).transient).toBe(true);
  });

  it("treats 4xx as NOT transient — retrying without changing input won't help", () => {
    expect(new ApiMutationError("Bad Request", 400).transient).toBe(false);
    expect(new ApiMutationError("Unauthorized", 401).transient).toBe(false);
    expect(new ApiMutationError("Forbidden", 403).transient).toBe(false);
    expect(new ApiMutationError("Not Found", 404).transient).toBe(false);
    expect(new ApiMutationError("Conflict", 409).transient).toBe(false);
    expect(new ApiMutationError("Unprocessable Entity", 422).transient).toBe(
      false,
    );
  });

  it("treats 3xx ambiguous as non-transient (server's saying redirect, not retry)", () => {
    expect(new ApiMutationError("Not Modified", 304).transient).toBe(false);
  });

  it("preserves the message and exposes the status", () => {
    const e = new ApiMutationError("nope", 503);
    expect(e.message).toBe("nope");
    expect(e.status).toBe(503);
    expect(e.name).toBe("ApiMutationError");
    expect(e).toBeInstanceOf(Error);
  });
});

describe("apiMutate error path", () => {
  it("unwraps the structured server error message", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(400, {
        error: { code: "validation", message: "Name is required" },
      }),
    );
    await expect(
      apiMutate({ path: "/me/collections", method: "POST", body: {} }),
    ).rejects.toThrow(
      expect.objectContaining({
        message: "Name is required",
        status: 400,
      }) as Error,
    );
  });

  it("falls back to the HTTP status when the body isn't JSON", async () => {
    mockedFetch.mockResolvedValueOnce(
      new Response("plaintext gateway error", {
        status: 502,
        headers: { "content-type": "text/plain" },
      }),
    );
    await expect(
      apiMutate({ path: "/anything", method: "POST" }),
    ).rejects.toThrow(
      expect.objectContaining({
        message: "502",
        status: 502,
      }) as Error,
    );
  });

  it("promotes a fetch rejection to status: 'network'", async () => {
    mockedFetch.mockRejectedValueOnce(new TypeError("Failed to fetch"));
    await expect(
      apiMutate({ path: "/anything", method: "POST" }),
    ).rejects.toThrow(
      expect.objectContaining({
        message: "Failed to fetch",
        status: "network",
      }) as Error,
    );
  });

  it("a 503 from the server is transient (Retry should attach)", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(503, {
        error: { code: "internal", message: "Service Unavailable" },
      }),
    );
    let caught: unknown;
    try {
      await apiMutate({ path: "/anything", method: "POST" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(ApiMutationError);
    expect((caught as ApiMutationError).transient).toBe(true);
  });

  it("a 409 from the server is NOT transient (Retry should NOT attach)", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(409, {
        error: {
          code: "conflict",
          message: "Already exists",
        },
      }),
    );
    let caught: unknown;
    try {
      await apiMutate({ path: "/anything", method: "POST" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(ApiMutationError);
    expect((caught as ApiMutationError).transient).toBe(false);
  });

  it("204 No Content returns null without throwing", async () => {
    mockedFetch.mockResolvedValueOnce(new Response(null, { status: 204 }));
    const result = await apiMutate({ path: "/anything", method: "DELETE" });
    expect(result).toBeNull();
  });
});
