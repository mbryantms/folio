/**
 * Shared toast strings.
 *
 * Anchor for messages emitted by 2+ call sites. Per the
 * [notifications audit](../../../docs/dev/notifications-audit.md) every
 * recurring string should live here so a copy edit doesn't drift across
 * three components. One-off strings (a unique success message, a
 * specific validation context) stay inline.
 *
 * If you find yourself reaching for a constant that doesn't exist yet,
 * grep the codebase first — a one-off string isn't a constant.
 */

export const TOAST = {
  /** The Want-to-Read collection is auto-seeded on first GET
   *  `/me/saved-views`. Briefly during sign-in the seed can lag the
   *  user's first add-to-WTR click; surfaced as a soft retry hint
   *  rather than a hard error since the next click usually succeeds. */
  WTR_NOT_READY: "Want to Read isn't ready yet — try again in a moment.",

  /** Required-field validation for name inputs (collections, views).
   *  Prefer pushing this into form-level validation so the message
   *  lands next to the field; the toast form is a fallback when the
   *  trigger is outside a form (cover-menu inline rename, etc.). */
  NAME_REQUIRED: "Name is required",

  /** Multi-page rails M6 — user-page CRUD completion messages. */
  PAGE_CREATED: "Page created",
  PAGE_RENAMED: "Page renamed",
  PAGE_DELETED: "Page deleted",
  PAGE_UPDATED: "Page updated",
} as const;
