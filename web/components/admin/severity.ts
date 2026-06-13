/** Health-issue severity vocabulary, shared between the per-library
 *  table and the cross-library findings view so the filter values can
 *  never drift from what the server emits (`health.rs` writes
 *  `"error" | "warning" | "info"` — a `"warn"` pill once shipped here
 *  and matched nothing). */
export const HEALTH_SEVERITIES = ["error", "warning", "info"] as const;

export type HealthSeverity = (typeof HEALTH_SEVERITIES)[number];

export type HealthSeverityFilter = HealthSeverity | "all";
