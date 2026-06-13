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

  it("parses error.details into the typed fields list on a 422", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(422, {
        error: {
          code: "validation",
          message: "port: must be 1-65535; host: required",
          details: [
            { field: "port", message: "must be 1-65535" },
            { field: "host", message: "required" },
          ],
        },
      }),
    );
    let caught: unknown;
    try {
      await apiMutate({ path: "/admin/email", method: "PATCH", body: {} });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(ApiMutationError);
    const err = caught as ApiMutationError;
    expect(err.fields).toEqual([
      { field: "port", message: "must be 1-65535" },
      { field: "host", message: "required" },
    ]);
  });

  it("leaves fields empty when the envelope has no details", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(400, {
        error: { code: "validation", message: "Name is required" },
      }),
    );
    let caught: unknown;
    try {
      await apiMutate({ path: "/me/collections", method: "POST", body: {} });
    } catch (e) {
      caught = e;
    }
    expect((caught as ApiMutationError).fields).toEqual([]);
  });

  it("drops malformed detail entries rather than trusting them", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(422, {
        error: {
          code: "validation",
          message: "bad",
          details: [
            { field: "ok", message: "fine" },
            { field: 123, message: "wrong-type" },
            "not-an-object",
            { message: "no field" },
          ],
        },
      }),
    );
    let caught: unknown;
    try {
      await apiMutate({ path: "/x", method: "POST", body: {} });
    } catch (e) {
      caught = e;
    }
    expect((caught as ApiMutationError).fields).toEqual([
      { field: "ok", message: "fine" },
    ]);
  });
});

describe("applyServerErrors", () => {
  it("calls setError per field error and returns true", async () => {
    const { applyServerErrors } = await import("@/lib/api/form-errors");
    const setError = vi.fn();
    const err = new ApiMutationError("bad", 422, [
      { field: "port", message: "must be 1-65535" },
      { field: "host", message: "required" },
    ]);
    const applied = applyServerErrors(setError, err);
    expect(applied).toBe(true);
    expect(setError).toHaveBeenCalledWith("port", {
      type: "server",
      message: "must be 1-65535",
    });
    expect(setError).toHaveBeenCalledWith("host", {
      type: "server",
      message: "required",
    });
  });

  it("routes empty-path and unknown fields to the form root", async () => {
    const { applyServerErrors } = await import("@/lib/api/form-errors");
    const setError = vi.fn();
    const err = new ApiMutationError("bad", 422, [
      { field: "", message: "whole-body rule failed" },
      { field: "stranger", message: "not on this form" },
    ]);
    applyServerErrors(setError, err, ["port", "host"]);
    expect(setError).toHaveBeenCalledWith("root.serverError", {
      type: "server",
      message: "whole-body rule failed",
    });
    expect(setError).toHaveBeenCalledWith("root.serverError", {
      type: "server",
      message: "stranger: not on this form",
    });
  });

  it("returns false and does nothing for non-422 errors", async () => {
    const { applyServerErrors } = await import("@/lib/api/form-errors");
    const setError = vi.fn();
    expect(applyServerErrors(setError, new ApiMutationError("x", 500))).toBe(
      false,
    );
    expect(applyServerErrors(setError, new Error("plain"))).toBe(false);
    expect(setError).not.toHaveBeenCalled();
  });
});
