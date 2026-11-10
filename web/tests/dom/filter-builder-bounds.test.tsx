// @vitest-environment jsdom
/**
 * Saved-view filter bounds in the builder: the server caps a filter at
 * `MAX_FILTER_CONDITIONS` conditions (422 with `error.details` on create /
 * update / preview) and answers 422 `filter_too_slow` when a filter query
 * hits its statement timeout. The builder stops adding conditions at the
 * cap, binds a save's field errors inline, and shows the too-slow
 * message from a preview as an inline notice instead of a spinner.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as Queries from "@/lib/api/queries";

const m = vi.hoisted(() => ({
  preview: vi.fn(),
  previewIssues: vi.fn(),
}));

vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<typeof Queries>()),
  previewSavedView: m.preview,
  previewSavedIssueView: m.previewIssues,
}));

import { FilterBuilder } from "@/components/filters/filter-builder";
import { HttpError } from "@/lib/api/queries";
import { ApiMutationError } from "@/lib/api/mutations/_core";
import type { Condition } from "@/lib/api/types";
import {
  FILTER_TOO_SLOW_CODE,
  FILTER_TOO_SLOW_MESSAGE,
  MAX_FILTER_CONDITIONS,
  filterQueryErrorMessage,
} from "@/lib/saved-views/filter-bounds";

function conditions(n: number): Condition[] {
  return Array.from({ length: n }, (_, i) => ({
    field: "name",
    op: "contains",
    value: `x${i}`,
  }));
}

function renderBuilder(
  n: number,
  onSave?: (s: unknown) => Promise<void>,
): void {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={qc}>
      <FilterBuilder
        initial={{ name: "Mine", conditions: conditions(n) }}
        onSave={onSave}
      />
    </QueryClientProvider>,
  );
}

const addButton = () =>
  screen.getByRole("button", { name: /add condition/i }) as HTMLButtonElement;

beforeEach(() => {
  m.preview.mockReset();
  m.previewIssues.mockReset();
});

describe("<FilterBuilder> bounds", () => {
  it("disables Add condition at the cap, with a hint", () => {
    renderBuilder(MAX_FILTER_CONDITIONS - 1);
    expect(addButton().disabled).toBe(false);
    expect(
      screen.queryByText(`Up to ${MAX_FILTER_CONDITIONS} conditions`),
    ).toBeNull();

    fireEvent.click(addButton());
    expect(addButton().disabled).toBe(true);
    expect(
      screen.getByText(`Up to ${MAX_FILTER_CONDITIONS} conditions`),
    ).toBeTruthy();
  });

  it("shows the server's field error from a 422 on save", async () => {
    const onSave = vi.fn().mockRejectedValue(
      new ApiMutationError(
        "filter.conditions: a filter can have at most 50 conditions",
        422,
        [
          {
            field: "filter.conditions",
            message: "a filter can have at most 50 conditions",
          },
          { field: "name", message: "name required" },
        ],
        "validation",
      ),
    );
    renderBuilder(3, onSave);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save view" }));
    });
    expect(onSave).toHaveBeenCalledOnce();
    expect(
      screen.getByText("a filter can have at most 50 conditions"),
    ).toBeTruthy();
    expect(screen.getByText("name required")).toBeTruthy();
    // The next edit clears the stale server errors.
    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "Renamed" },
    });
    expect(
      screen.queryByText("a filter can have at most 50 conditions"),
    ).toBeNull();
  });

  it("shows the too-slow notice from a preview instead of a spinner", async () => {
    m.preview.mockRejectedValue(
      new HttpError(422, FILTER_TOO_SLOW_MESSAGE, FILTER_TOO_SLOW_CODE),
    );
    renderBuilder(2);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    });
    expect(m.preview).toHaveBeenCalledOnce();
    expect(screen.getByRole("alert").textContent).toBe(FILTER_TOO_SLOW_MESSAGE);
    expect(screen.queryByText("Previewing…")).toBeNull();
  });
});

describe("filterQueryErrorMessage", () => {
  it("maps the time-box code to the notice, other 422s to their message", () => {
    expect(
      filterQueryErrorMessage(new HttpError(422, "x", FILTER_TOO_SLOW_CODE)),
    ).toBe(FILTER_TOO_SLOW_MESSAGE);
    expect(filterQueryErrorMessage(new HttpError(422, "bad filter"))).toBe(
      "bad filter",
    );
    expect(filterQueryErrorMessage(new HttpError(500, "internal"))).toBe(
      "Couldn't load this view. Try again later.",
    );
  });
});
