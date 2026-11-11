// @vitest-environment jsdom
/**
 * Admin user → Library access (audit U-09): the age-cap picker lists every
 * ladder rung under a group heading, each library has a "Hide unrated"
 * switch that is sent as the `hide_unrated` map, and the "what this hides"
 * line follows the selected cap and switch before saving.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AGE_RATING_LADDER } from "@/lib/age-rating";
import type {
  AdminUserDetailView,
  LibraryAgeRatingsView,
  LibraryView,
} from "@/lib/api/types";

const LIB_A = "00000000-0000-0000-0000-00000000000a";
const LIB_B = "00000000-0000-0000-0000-00000000000b";

const mutate = vi.fn();
vi.mock("@/lib/api/mutations", () => ({
  useUpdateLibraryAccess: () => ({ mutate, isPending: false }),
}));

const libraries = [
  { id: LIB_A, name: "Comics", root_path: "/comics" },
  { id: LIB_B, name: "Manga", root_path: "/manga" },
] as unknown as LibraryView[];

// Library A: 2 Teen, 3 Mature 17+, 1 X18+, 4 unrated (10 series).
const counts: Record<string, number> = { Teen: 2, "Mature 17+": 3, "X18+": 1 };
const ageRatings: LibraryAgeRatingsView[] = [
  {
    library_id: LIB_A,
    rungs: AGE_RATING_LADDER.map((rating) => ({
      rating,
      series_count: counts[rating] ?? 0,
    })),
    unrated_count: 4,
    series_count: 10,
  },
];

vi.mock("@/lib/api/queries", () => ({
  useLibraryList: () => ({ data: libraries, isLoading: false }),
  useLibraryAgeRatingsAll: () => ({ data: ageRatings }),
}));

import { LibraryAccessMatrix } from "@/components/admin/users/LibraryAccessMatrix";

function user(
  access: { library_id: string; cap: string | null; hide: boolean }[],
): AdminUserDetailView {
  return {
    id: "u1",
    email: "reader@example.com",
    display_name: "Reader",
    role: "user",
    state: "active",
    email_verified: true,
    created_at: "2026-01-01T00:00:00Z",
    last_login_at: null,
    library_count: access.length,
    library_access: access.map((a) => ({
      library_id: a.library_id,
      library_name: a.library_id === LIB_A ? "Comics" : "Manga",
      age_rating_max: a.cap,
      hide_unrated: a.hide,
    })),
  } as AdminUserDetailView;
}

afterEach(() => {
  cleanup();
  mutate.mockReset();
});

const capSelect = (name: string) =>
  screen.getByRole("combobox", {
    name: `Age-rating cap for ${name}`,
  }) as HTMLSelectElement;
const hideSwitch = (name: string) =>
  screen.getByRole("switch", { name: `Hide unrated series in ${name}` });
const save = () => screen.getByRole("button", { name: "Save changes" });

describe("<LibraryAccessMatrix> age cap (U-09)", () => {
  it("lists No cap first, then every rung under its group heading", () => {
    render(
      <LibraryAccessMatrix
        user={user([{ library_id: LIB_A, cap: null, hide: false }])}
      />,
    );
    const select = capSelect("Comics");
    expect(select.options.item(0)?.textContent).toBe("No cap");
    expect(select.options.item(0)?.value).toBe("");
    const groups = Array.from(select.querySelectorAll("optgroup"));
    expect(groups.map((g) => g.label)).toEqual([
      "Young children",
      "All ages",
      "Older children",
      "Teens",
      "Mature",
      "Adults only",
    ]);
    const values = groups.flatMap((g) =>
      Array.from(g.querySelectorAll("option")).map((o) => o.value),
    );
    expect(values).toEqual([...AGE_RATING_LADDER]);
  });

  it.each(AGE_RATING_LADDER)(
    "an existing %s grant displays and saves unchanged",
    (rung) => {
      render(
        <LibraryAccessMatrix
          user={user([
            { library_id: LIB_A, cap: rung, hide: true },
            { library_id: LIB_B, cap: null, hide: false },
          ])}
        />,
      );
      expect(capSelect("Comics").value).toBe(rung);
      expect(hideSwitch("Comics").getAttribute("aria-checked")).toBe("true");
      expect((save() as HTMLButtonElement).disabled).toBe(true);
      // Change something else; the loaded grant goes back as it came.
      fireEvent.click(hideSwitch("Manga"));
      fireEvent.click(save());
      expect(mutate).toHaveBeenCalledWith({
        library_ids: [LIB_A, LIB_B],
        age_rating_caps: { [LIB_A]: rung },
        hide_unrated: { [LIB_A]: true, [LIB_B]: true },
      });
    },
  );

  it("toggling the switch sends the hide_unrated map", () => {
    render(
      <LibraryAccessMatrix
        user={user([{ library_id: LIB_A, cap: "Teen", hide: false }])}
      />,
    );
    const sw = hideSwitch("Comics");
    expect(sw.getAttribute("aria-checked")).toBe("false");
    // The one-sentence explanation is the switch's description.
    const help = document.getElementById(
      sw.getAttribute("aria-describedby") ?? "",
    );
    expect(help?.textContent).toMatch(/with or without a cap/);
    sw.focus();
    expect(document.activeElement).toBe(sw);
    fireEvent.click(sw);
    expect(sw.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(save());
    expect(mutate).toHaveBeenCalledWith({
      library_ids: [LIB_A],
      age_rating_caps: { [LIB_A]: "Teen" },
      hide_unrated: { [LIB_A]: true },
    });
  });

  it("the switch is disabled for a library without access", () => {
    render(
      <LibraryAccessMatrix
        user={user([{ library_id: LIB_A, cap: null, hide: false }])}
      />,
    );
    expect(hideSwitch("Manga").hasAttribute("disabled")).toBe(true);
    expect(capSelect("Manga").disabled).toBe(true);
  });

  it("the summary follows the cap and the switch before saving", () => {
    render(
      <LibraryAccessMatrix
        user={user([{ library_id: LIB_A, cap: null, hide: false }])}
      />,
    );
    const line = () => screen.getByTestId(`cap-effect-${LIB_A}`).textContent;
    expect(line()).toBe("Hides none of 10 series; 4 unrated still shown.");

    fireEvent.change(capSelect("Comics"), { target: { value: "Teen" } });
    expect(line()).toBe("Hides 4 of 10 series; 4 unrated still shown.");
    // The chosen rung's group explains who it suits.
    expect(screen.getByText("Teens:")).toBeTruthy();

    fireEvent.click(hideSwitch("Comics"));
    expect(line()).toBe("Hides 8 of 10 series; 4 unrated hidden.");

    fireEvent.change(capSelect("Comics"), { target: { value: "" } });
    expect(line()).toBe("Hides 4 of 10 series; 4 unrated hidden.");
    expect(mutate).not.toHaveBeenCalled();
  });
});
