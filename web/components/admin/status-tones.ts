/**
 * Admin status → tone rules, one table per vocabulary, so every admin
 * surface draws the same state the same way.
 *
 * The rule: a healthy, ordinary state (Complete, Active, Idle, OK,
 * Cancelled by an admin) is `neutral`; work under way (Running, Queued) is
 * `info`, distinct from both; only a state that needs the admin's
 * attention takes `warning` or `error`. A page of healthy rows then reads
 * as quiet, and the rows that need a look stand out. `success` is not
 * used for status here: a green "Complete" on every row is the same noise
 * as an amber one.
 *
 * Tones resolve through `lib/ui/status-tone.ts` (theme tokens only).
 */
import type { StatusTone } from "@/lib/ui/status-tone";

/** Scan run (`ScanRunState`) and the live scan's own status words. */
export function scanRunStateTone(state: string): StatusTone {
  switch (state) {
    case "running":
    case "queued":
    case "thumbnailing":
      return "info";
    case "failed":
      return "error";
    // complete / completed / cancelled (an admin's own action) and
    // anything unknown read as ordinary.
    default:
      return "neutral";
  }
}

/** "Scan all" batch (`ScanBatchView.state`). */
export function scanBatchStateTone(state: string): StatusTone {
  switch (state) {
    case "running":
      return "info";
    case "partial_failed":
      return "warning";
    case "failed":
      return "error";
    default:
      return "neutral";
  }
}

/** User account state (`UserState`). Pending verification cannot sign in
 *  until it is resolved; disabled is blocked outright. */
export function userStateTone(state: string): StatusTone {
  switch (state) {
    case "pending_verification":
      return "warning";
    case "disabled":
      return "error";
    default:
      return "neutral";
  }
}

/** Background / metadata job state. */
export function jobStateTone(state: string): StatusTone {
  switch (state) {
    case "running":
    case "queued":
    case "pending":
      return "info";
    case "awaiting_quota":
    case "partial_failed":
      return "warning";
    case "failed":
      return "error";
    default:
      return "neutral";
  }
}

/** Health finding severity (`HEALTH_SEVERITIES`). `info` is advisory. */
export function healthSeverityTone(severity: string): StatusTone {
  switch (severity) {
    case "error":
      return "error";
    case "warning":
      return "warning";
    default:
      return "neutral";
  }
}

/** A service up / down check (dashboard health pills). */
export function serviceCheckTone(ok: boolean): StatusTone {
  return ok ? "neutral" : "error";
}
