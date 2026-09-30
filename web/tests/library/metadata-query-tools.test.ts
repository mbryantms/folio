/**
 * `overridesFromForm` — the pure builder behind the match dialog's
 * "Adjust query" form (WP-2.8). Only fields that differ from what the
 * current run searched are sent, so the server's `overridden` flag
 * (and the "Searched as …" line it drives) means what it says.
 */
import { describe, expect, it } from "vitest";

import { overridesFromForm } from "@/components/library/MetadataQueryTools";
import type { SearchQueryView } from "@/lib/api/types";

const current: SearchQueryView = {
  kind: "series",
  name: "Saga",
  year: 2012,
  publisher: "Image Comics",
  label: "Saga",
  overridden: false,
  year_gate_relaxed: false,
  lookup: false,
};

describe("overridesFromForm", () => {
  it("returns null when nothing differs from the current run", () => {
    expect(
      overridesFromForm(
        {
          name: "Saga",
          year: "2012",
          publisher: "Image Comics",
          issue_number: "",
        },
        current,
        false,
      ),
    ).toBeNull();
  });

  it("sends only the changed fields, trimmed, with year as a number", () => {
    expect(
      overridesFromForm(
        {
          name: "  Saga Deluxe ",
          year: " 2014 ",
          publisher: "Image Comics",
          issue_number: "",
        },
        current,
        false,
      ),
    ).toEqual({ name: "Saga Deluxe", year: 2014 });
  });

  it("drops blank inputs and non-numeric years instead of sending them", () => {
    expect(
      overridesFromForm(
        { name: "", year: "abc", publisher: "  ", issue_number: "" },
        current,
        false,
      ),
    ).toBeNull();
  });

  it("includes issue_number only for issue scope", () => {
    const form = {
      name: "Saga",
      year: "2012",
      publisher: "Image Comics",
      issue_number: "Annual 1",
    };
    expect(overridesFromForm(form, current, false)).toBeNull();
    expect(overridesFromForm(form, current, true)).toEqual({
      issue_number: "Annual 1",
    });
  });

  it("treats every non-empty field as an override before the run is known", () => {
    expect(
      overridesFromForm(
        { name: "Saga", year: "2012", publisher: "", issue_number: "" },
        null,
        false,
      ),
    ).toEqual({ name: "Saga", year: 2012 });
  });
});
