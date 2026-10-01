/**
 * Marker colour palette (roadmap WP-5.3, owner decision 2026-09-30).
 *
 * `marker.color` is one of these names or a `#RRGGBB` / `#RRGGBBAA` hex;
 * the server rejects anything else with a 422 on `color`. Nothing renders
 * marker colour yet — this is the shared definition for when something
 * does. The server's source of truth is `MARKER_PALETTE` in
 * `crates/server/src/api/markers.rs`, published as the OpenAPI `pattern`
 * on `CreateMarkerReq.color`; `web/tests/markers/palette.test.ts` checks
 * this list against that pattern so the two can't drift.
 */
export const MARKER_PALETTE = [
  "yellow",
  "green",
  "blue",
  "red",
  "violet",
] as const;

export type MarkerPaletteColor = (typeof MARKER_PALETTE)[number];

const HEX = /^#(?:[0-9A-Fa-f]{6}|[0-9A-Fa-f]{8})$/;

/** Whether `value` is a colour the server accepts (blank = no colour). */
export function isValidMarkerColor(value: string): boolean {
  const v = value.trim();
  return (
    v === "" || (MARKER_PALETTE as readonly string[]).includes(v) || HEX.test(v)
  );
}
