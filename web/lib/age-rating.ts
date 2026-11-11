/**
 * ComicInfo `AgeRating` ladder, youngest-audience-first. Mirrors
 * `crates/server/src/library/age_rating.rs::LADDER` — the server is the
 * source of truth (it validates `age_rating_caps` against the same list and
 * stores the canonical spelling); this copy drives the admin picker and the
 * "what this cap hides" arithmetic. `crates/server/tests/library_age_ratings.rs`
 * fails when the two lists differ.
 */
export const AGE_RATING_LADDER = [
  "Early Childhood",
  "Everyone",
  "G",
  "Everyone 10+",
  "PG",
  "Kids to Adults",
  "Teen",
  "MA15+",
  "Mature 17+",
  "M",
  "R18+",
  "Adults Only 18+",
  "X18+",
] as const;

export type AgeRating = (typeof AGE_RATING_LADDER)[number];

/** A run of adjacent ladder rungs under one plain heading. */
export interface AgeRatingGroup {
  heading: string;
  /** Who a cap in this group suits, in plain words. */
  description: string;
  /** The group's rungs, in ladder order. */
  ratings: readonly AgeRating[];
}

/**
 * Where each group starts on the ladder. A group runs from its first rung
 * up to the rung before the next group's first rung, so every rung falls
 * in exactly one group, in ladder order, whatever the ladder holds.
 */
const GROUP_STARTS: readonly {
  first: AgeRating;
  heading: string;
  description: string;
}[] = [
  {
    first: "Early Childhood",
    heading: "Young children",
    description: "Made for pre-school and early readers.",
  },
  {
    first: "Everyone",
    heading: "All ages",
    description: "Fine for any reader, with nothing a parent needs to check.",
  },
  {
    first: "Everyone 10+",
    heading: "Older children",
    description:
      "Around 10 and up: cartoon action and mild themes a younger child may find scary.",
  },
  {
    first: "Teen",
    heading: "Teens",
    description:
      "Around 13 and up (MA15+ is 15 and up): action violence, some language and suggestive themes.",
  },
  {
    first: "Mature 17+",
    heading: "Mature",
    description:
      "Around 17 and up: strong violence, language and sexual themes.",
  },
  {
    first: "R18+",
    heading: "Adults only",
    description: "18 and up: explicit content.",
  },
];

export const AGE_RATING_GROUPS: readonly AgeRatingGroup[] = GROUP_STARTS.map(
  (g, i) => {
    const from = AGE_RATING_LADDER.indexOf(g.first);
    const next = GROUP_STARTS[i + 1];
    const to = next
      ? AGE_RATING_LADDER.indexOf(next.first)
      : AGE_RATING_LADDER.length;
    return {
      heading: g.heading,
      description: g.description,
      ratings: AGE_RATING_LADDER.slice(from, to),
    };
  },
);

/** Rank of a ladder rung (its index), or `null` for anything else. */
export function ladderRank(rating: string | null | undefined): number | null {
  if (!rating) return null;
  const i = (AGE_RATING_LADDER as readonly string[]).indexOf(rating);
  return i < 0 ? null : i;
}

/** The group a rung belongs to; `null` for a value not on the ladder. */
export function groupOf(
  rating: string | null | undefined,
): AgeRatingGroup | null {
  if (ladderRank(rating) === null) return null;
  return (
    AGE_RATING_GROUPS.find((g) =>
      (g.ratings as readonly (string | null | undefined)[]).includes(rating),
    ) ?? null
  );
}
