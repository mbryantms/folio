/**
 * ComicInfo `AgeRating` ladder, youngest-audience-first. Mirrors
 * `crates/server/src/library/age_rating.rs::LADDER` — the server is the
 * source of truth (it validates `age_rating_caps` against the same list and
 * stores the canonical spelling); this copy only drives the admin picker.
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
