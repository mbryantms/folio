// @vitest-environment jsdom
/**
 * Audit 2026-11 N-A4: `POST /me/collections` validates through garde, so a
 * 422 carries `error.details` naming the field. The new-collection dialog
 * keeps its values in component state (no react-hook-form), and binds
 * those details under the inputs through `collectServerErrors`; the
 * mutation hook still toasts the summary.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  create: vi.fn(),
}));

vi.mock("@/lib/api/queries", () => ({
  useCollections: () => ({ data: undefined, isLoading: false }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useCreateCollection: () => ({ mutate: m.create, isPending: false }),
}));

import { NewCollectionDialog } from "@/components/collections/CollectionsIndex";
import { collectServerErrors } from "@/lib/api/form-errors";
import { ApiMutationError } from "@/lib/api/mutations/_core";

beforeEach(() => {
  m.create.mockReset();
});

describe("collectServerErrors", () => {
  it("keys the server's details by field and parks the rest under root", () => {
    const err = new ApiMutationError("name: name required; bad", 422, [
      { field: "name", message: "name required" },
      { field: "colour", message: "unknown" },
      { field: "", message: "bad" },
    ]);
    expect(collectServerErrors(err, ["name", "description"])).toEqual({
      name: "name required",
      root: "bad",
    });
  });

  it("is empty for anything that is not a field-level 422", () => {
    expect(collectServerErrors(new Error("boom"), ["name"])).toEqual({});
    expect(
      collectServerErrors(new ApiMutationError("nope", 409), ["name"]),
    ).toEqual({});
  });
});

describe("<NewCollectionDialog>", () => {
  it("binds a server 422 to the field it names", async () => {
    m.create.mockImplementation(
      (_input: unknown, opts: { onError: (e: unknown) => void }) =>
        opts.onError(
          new ApiMutationError("name: name must be 200 chars or fewer", 422, [
            { field: "name", message: "name must be 200 chars or fewer" },
          ]),
        ),
    );
    render(<NewCollectionDialog open onOpenChange={() => {}} />);
    const input = screen.getByLabelText("Name");
    fireEvent.change(input, { target: { value: "Capes" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Create" }));
    });
    expect(m.create).toHaveBeenCalledOnce();
    expect(m.create.mock.calls[0]![0]).toEqual({
      name: "Capes",
      description: "",
    });
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(input.getAttribute("aria-describedby")).toBe(
      "new-collection-name-error",
    );
    expect(screen.getByText("name must be 200 chars or fewer")).toBeTruthy();
    // The description input is untouched by a name error.
    expect(
      screen.getByLabelText("Description").getAttribute("aria-invalid"),
    ).toBeNull();
  });

  it("clears the bound error on the next submit", async () => {
    let calls = 0;
    m.create.mockImplementation(
      (_input: unknown, opts: { onError: (e: unknown) => void }) => {
        calls += 1;
        if (calls === 1) {
          opts.onError(
            new ApiMutationError("name: name required", 422, [
              { field: "name", message: "name required" },
            ]),
          );
        }
      },
    );
    render(<NewCollectionDialog open onOpenChange={() => {}} />);
    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "X" },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Create" }));
    });
    expect(screen.getByText("name required")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Create" }));
    });
    expect(screen.queryByText("name required")).toBeNull();
    expect(
      screen.getByLabelText("Name").getAttribute("aria-invalid"),
    ).toBeNull();
  });
});
