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
  /**
   * Who a cap in this group suits, in plain words. Written for the most
   * permissive rung of the group, so it never promises milder content
   * than a rung under it allows.
   */
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
    description:
      "General audiences; may still have mild cartoon violence and mild language.",
  },
  {
    first: "Everyone 10+",
    heading: "Older children",
    description:
      "Around 10 and up, or younger with a parent: more cartoon or fantasy violence, mild language, some suggestive themes.",
  },
  {
    first: "Teen",
    heading: "Teens",
    description:
      "Around 13 and up: violence, crude humour, some strong language and suggestive themes.",
  },
  {
    first: "MA15+",
    heading: "Older teens",
    description:
      "15 and up, a legally restricted category in Australia: strong violence, drug use, nudity and coarse language. Metron's Teen Plus counts here.",
  },
  {
    first: "Mature 17+",
    heading: "Mature",
    description:
      "Around 17 and up: intense violence, blood and gore, strong language and sexual content.",
  },
  {
    first: "R18+",
    heading: "Adults only",
    description: "18 and up: high-impact violence and explicit sexual content.",
  },
];

/** One line for the legend: where the ratings come from. */
export const AGE_RATING_GROUPS_NOTE =
  "The ratings come from several rating systems (ESRB, Australian classification, Metron and others). The headings and descriptions are a guide, not a ruling on any one book.";

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

/**
 * Characters stripped from both ends of a cap before it is compared: the
 * server's `age_rating::RATING_TRIM` (ASCII space, tab, line feed, carriage
 * return, form feed, vertical tab). Not `String.trim`, which strips more.
 */
const RATING_TRIM = new Set([" ", "\t", "\n", "\r", "\f", "\v"]);

function trimRating(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && RATING_TRIM.has(value.charAt(start))) start++;
  while (end > start && RATING_TRIM.has(value.charAt(end - 1))) end--;
  return value.slice(start, end);
}

/**
 * The ladder rung a stored cap names, in its canonical spelling, the way
 * the server reads a grant's cap (`age_rating::is_valid_cap`): case and
 * surrounding whitespace are ignored. `null` for anything that is not a
 * rung — an alias such as `Mature`, an unrated word or garbage — which the
 * server does not enforce as a cap.
 */
export function canonicalCap(
  stored: string | null | undefined,
): AgeRating | null {
  if (!stored) return null;
  const needle = trimRating(stored).toLowerCase();
  return AGE_RATING_LADDER.find((r) => r.toLowerCase() === needle) ?? null;
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
