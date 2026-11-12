// @vitest-environment jsdom
/**
 * Admin server page: the OCR models card and the server info card show
 * their error message when the request fails (D8). They used to check
 * `!data` before `error`, which is also true on error, so the skeleton
 * stayed up for good.
 */
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import type * as Queries from "@/lib/api/queries";

const { state } = vi.hoisted(() => ({
  state: {
    models: {} as Record<string, unknown>,
    info: {} as Record<string, unknown>,
    modelsOpts: undefined as unknown,
  },
}));
vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<typeof Queries>()),
  useOcrModels: (opts: unknown) => {
    state.modelsOpts = opts;
    return state.models;
  },
  useServerInfo: () => state.info,
  useLatestRelease: () => ({ data: undefined }),
}));

import {
  OcrModelsCard,
  ServerInfoClient,
} from "@/components/admin/observability/ServerInfoClient";

describe("admin OCR models card", () => {
  it("shows its error when the request fails", () => {
    state.models = {
      data: undefined,
      error: new Error("boom"),
      isLoading: false,
      isError: true,
    };
    render(<OcrModelsCard />);
    expect(screen.getByText("Failed to load OCR model status.")).toBeTruthy();
  });

  it("polls every 30 seconds, as its comment says", () => {
    state.models = { data: undefined, error: null, isLoading: true };
    render(<OcrModelsCard />);
    expect(state.modelsOpts).toEqual({ intervalMs: 30_000 });
  });

  it("the server info card shows its error too", () => {
    state.info = {
      data: undefined,
      error: new Error("boom"),
      isLoading: false,
      isError: true,
    };
    render(<ServerInfoClient />);
    expect(screen.getByText("Failed to load server info.")).toBeTruthy();
  });
});
