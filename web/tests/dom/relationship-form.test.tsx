// @vitest-environment jsdom
/**
 * WP-7.7 relationship editing: the searchable grouped kind picker
 * (`RelationshipKindSelect`) and the add / edit form
 * (`RelationshipForm`) — scope fields per kind, the PATCH body, the
 * Series | Story arc target toggle, and server 422 field errors bound to
 * their inputs via `applyServerErrors`.
 */
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { RelationshipCatalogue } from "@/lib/api/types";

const m = vi.hoisted(() => ({
  catalogue: undefined as unknown,
  seriesPages: undefined as unknown,
  update: vi.fn(),
  create: vi.fn(),
}));

vi.mock("@/lib/api/queries", () => ({
  useRelationshipKinds: () => ({ data: m.catalogue }),
  useSeriesListInfinite: () => ({ data: m.seriesPages, isLoading: false }),
  useEntityListInfinite: () => ({ data: undefined, isLoading: false }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useUpdateSeriesRelationship: () => ({ mutate: m.update, isPending: false }),
  useCreateSeriesRelationship: () => ({ mutate: m.create, isPending: false }),
}));

import { RelationshipKindSelect } from "@/components/library/RelationshipKindSelect";
import {
  RelationshipForm,
  TargetPicker,
  patchBody,
} from "@/components/library/RelationshipFormDialog";
import { ApiMutationError } from "@/lib/api/mutations/_core";

const q = (value: string, label: string) => ({ value, label });
function kind(
  k: string,
  label: string,
  group: string,
  extra: Partial<{
    qualifiers: Array<{ value: string; label: string }>;
    allows_coverage: boolean;
    allows_arc_target: boolean;
  }> = {},
) {
  return {
    kind: k,
    inverse: k,
    label,
    inverse_label: label,
    group,
    symmetric: false,
    qualifiers: [],
    allows_coverage: false,
    allows_arc_target: false,
    ...extra,
  };
}

const CATALOGUE = {
  groups: [
    { group: "story", label: "Story" },
    { group: "publication", label: "Publication history" },
    { group: "editions", label: "Editions & contents" },
    { group: "advanced", label: "Advanced" },
  ],
  kinds: [
    kind("sequel_of", "Sequel to", "story"),
    kind("tie_in_to", "Tie-in to", "story", {
      allows_arc_target: true,
      qualifiers: [
        q("main", "Main story"),
        q("tie_in", "Tie-in"),
        q("prelude", "Prelude"),
        q("aftermath", "Aftermath"),
      ],
    }),
    kind("continues", "Continues", "publication", {
      qualifiers: [q("relaunch", "Relaunch"), q("retitle", "Retitle")],
    }),
    kind("collects", "Collects", "editions", { allows_coverage: true }),
    kind("collected_in", "Collected in", "editions", {
      allows_coverage: true,
    }),
    kind("adaptation_of", "Adaptation of", "advanced"),
  ],
} as unknown as RelationshipCatalogue;

beforeEach(() => {
  m.catalogue = CATALOGUE;
  m.seriesPages = undefined;
  m.update.mockReset();
  m.create.mockReset();
});

function openPicker(name = "Relationship") {
  fireEvent.click(screen.getByRole("combobox", { name }));
}

describe("<RelationshipKindSelect>", () => {
  it("shows a skeleton, not a raw key, until the catalogue loads", () => {
    m.catalogue = undefined;
    render(<RelationshipKindSelect value="sequel_of" onChange={() => {}} />);
    expect(screen.getByTestId("relationship-kind-loading")).toBeTruthy();
    expect(screen.queryByText("sequel_of")).toBeNull();
    expect(screen.queryByText("sequel of")).toBeNull();
  });

  it("lists every group with a heading and filters by search", () => {
    const onChange = vi.fn();
    render(<RelationshipKindSelect value="sequel_of" onChange={onChange} />);
    expect(screen.getByRole("combobox").textContent).toContain("Sequel to");
    openPicker();
    for (const g of [
      "Story",
      "Publication history",
      "Editions & contents",
      "Advanced",
    ]) {
      expect(screen.getByText(g)).toBeTruthy();
    }
    fireEvent.change(screen.getByPlaceholderText("Search relationships…"), {
      target: { value: "collect" },
    });
    expect(screen.getByRole("option", { name: /Collects/ })).toBeTruthy();
    expect(screen.queryByRole("option", { name: /Sequel to/ })).toBeNull();
    // A group name matches its kinds too.
    fireEvent.change(screen.getByPlaceholderText("Search relationships…"), {
      target: { value: "advanced" },
    });
    expect(screen.getByRole("option", { name: /Adaptation of/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("option", { name: /Adaptation of/ }));
    expect(onChange).toHaveBeenCalledWith("adaptation_of");
  });

  it("scrolls in one themed area and opens on the current kind", () => {
    render(<RelationshipKindSelect value="collects" onChange={() => {}} />);
    openPicker();
    const area = screen.getByTestId("relationship-kind-scroll");
    // The listbox lives inside the ScrollArea; the popover itself
    // doesn't scroll (no second scrollbar).
    expect(area.querySelector("[cmdk-list]")).toBeTruthy();
    const popover = area.closest("[data-radix-popper-content-wrapper] > *");
    expect(popover?.className).toContain("overflow-hidden");
    expect(
      screen
        .getByRole("option", { name: /Collects/ })
        .getAttribute("aria-selected"),
    ).toBe("true");
  });

  it("can restrict the choices", () => {
    render(
      <RelationshipKindSelect
        value="tie_in_to"
        onChange={() => {}}
        filter={(k) => k === "tie_in_to"}
      />,
    );
    openPicker();
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      "Tie-in to",
    ]);
  });
});

describe("<RelationshipForm> edit", () => {
  const edit = {
    id: "rel-1",
    kind: "sequel_of" as const,
    isArc: false,
    otherName: "Saga Vol 1",
    from_range: "1-6",
    note: "old note",
  };

  it("shows only the scope the kind takes and sends a full PATCH body", async () => {
    render(
      <RelationshipForm
        seriesSlug="saga-2"
        seriesId="s2"
        edit={edit}
        onDone={() => {}}
      />,
    );
    expect(
      (screen.getByLabelText("This series’ issues") as HTMLInputElement).value,
    ).toBe("1-6");
    expect(screen.queryByText("Coverage")).toBeNull();
    expect(screen.queryByText("Qualifier")).toBeNull();
    // No target picker when editing.
    expect(screen.queryByRole("radiogroup", { name: "Target" })).toBeNull();

    openPicker();
    fireEvent.click(screen.getByRole("option", { name: /Collected in/ }));
    expect(screen.getByText("Coverage")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Their issues"), {
      target: { value: " 1-12 " },
    });
    fireEvent.change(screen.getByLabelText("Note"), { target: { value: "" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });
    expect(m.update).toHaveBeenCalledOnce();
    expect(m.update.mock.calls[0]![0]).toEqual({
      id: "rel-1",
      body: {
        kind: "collected_in",
        qualifier: null,
        coverage: null,
        from_range: "1-6",
        to_range: "1-12",
        note: null,
      },
    });
  });

  it("binds a server 422 to the field it names", async () => {
    m.update.mockImplementation(
      (_input: unknown, opts: { onError: (e: unknown) => void }) =>
        opts.onError(
          new ApiMutationError("to_range: too long", 422, [
            { field: "to_range", message: "must be at most 100 characters" },
          ]),
        ),
    );
    render(
      <RelationshipForm
        seriesSlug="saga-2"
        seriesId="s2"
        edit={edit}
        onDone={() => {}}
      />,
    );
    fireEvent.change(screen.getByLabelText("Note"), {
      target: { value: "new" },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });
    const input = screen.getByLabelText("Their issues");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByText("must be at most 100 characters")).toBeTruthy();
  });

  it("keeps an arc edge on arc-capable kinds", () => {
    render(
      <RelationshipForm
        seriesSlug="saga-2"
        seriesId="s2"
        edit={{
          id: "a1",
          kind: "tie_in_to",
          isArc: true,
          otherName: "Secret Wars",
          qualifier: "prelude",
        }}
        onDone={() => {}}
      />,
    );
    expect(screen.getByText("Role")).toBeTruthy();
    openPicker();
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      "Tie-in to",
    ]);
  });

  it("drops scope a new kind doesn't accept", () => {
    expect(
      patchBody(
        {
          kind: "sequel_of",
          target_type: "series",
          target: null,
          target_arc: null,
          qualifier: "relaunch",
          coverage: "full",
          from_range: "",
          to_range: "",
          note: "  ",
        },
        CATALOGUE,
      ),
    ).toEqual({
      kind: "sequel_of",
      qualifier: null,
      coverage: null,
      from_range: null,
      to_range: null,
      note: null,
    });
  });
});

describe("<RelationshipForm> add", () => {
  it("offers a story-arc target only for tie-ins", async () => {
    render(
      <RelationshipForm seriesSlug="saga-2" seriesId="s2" onDone={() => {}} />,
    );
    const arc = screen.getByRole("radio", { name: "Story arc" });
    expect((arc as HTMLButtonElement).disabled).toBe(true);
    openPicker();
    fireEvent.click(screen.getByRole("option", { name: /Tie-in to/ }));
    expect(
      (screen.getByRole("radio", { name: "Story arc" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
    fireEvent.click(screen.getByRole("radio", { name: "Story arc" }));
    expect(
      screen.getByRole("button", { name: "Choose a story arc" }),
    ).toBeTruthy();
    // Submitting without a target flags the arc picker, no request.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Add" }));
    });
    expect(m.create).not.toHaveBeenCalled();
    expect(screen.getByText("Choose a story arc")).toBeTruthy();
  });
});

describe("<TargetPicker>", () => {
  it("is a keyboard-navigable command list in the themed scroll area", async () => {
    m.seriesPages = {
      pages: [
        {
          items: [
            { id: "s1", name: "Saga", year: 2012, publisher: "Image" },
            { id: "s9", name: "Saga Deluxe", year: 2020, publisher: "Image" },
          ],
        },
      ],
    };
    const onChange = vi.fn();
    render(<TargetPicker kind="series" value={null} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a series" }));
    const input = screen.getByPlaceholderText("Search series…");
    fireEvent.change(input, { target: { value: "saga" } });
    // The search is debounced; wait for the options instead of sleeping.
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(2));
    // Rows are cmdk options inside the ScrollArea (one scroller, themed
    // scrollbar), not hand-rolled buttons in a native `overflow-auto`.
    const scroll = screen.getByTestId("relationship-target-scroll");
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(2);
    for (const o of options) expect(scroll.contains(o)).toBe(true);
    // Arrow keys move the highlight; Enter picks it.
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ id: "s9", name: "Saga Deluxe (2020)" }),
    );
  });
});
