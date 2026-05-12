import cronstrue from "cronstrue";
import { CronExpressionParser } from "cron-parser";

export type CronValidation =
  | { ok: true; humanized: string; nextRuns: Date[] }
  | { ok: false; error: string };

/**
 * Validate a cron expression and produce a human-readable preview plus the
 * next 3 occurrences. Empty / whitespace-only input is treated as "no schedule"
 * (`ok: true` with humanized: "Never").
 */
export function validateCron(
  expression: string | null | undefined,
): CronValidation {
  const trimmed = (expression ?? "").trim();
  if (trimmed === "") {
    return { ok: true, humanized: "Never (no schedule)", nextRuns: [] };
  }
  const nextRuns: Date[] = [];
  try {
    const interval = CronExpressionParser.parse(trimmed);
    for (let i = 0; i < 3; i++) {
      nextRuns.push(interval.next().toDate());
    }
  } catch (e) {
    return {
      ok: false,
      error: e instanceof Error ? e.message : "Invalid cron",
    };
  }
  let humanized: string;
  try {
    humanized = cronstrue.toString(trimmed, { use24HourTimeFormat: true });
  } catch {
    humanized = trimmed;
  }
  return { ok: true, humanized, nextRuns };
}
