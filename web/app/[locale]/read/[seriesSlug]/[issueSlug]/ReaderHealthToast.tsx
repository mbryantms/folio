"use client";

import { useEffect, useRef } from "react";
import { toast } from "sonner";

import { useIssueHealth } from "@/lib/api/queries";
import type { HealthIssueView } from "@/lib/api/types";

const STORAGE_PREFIX = "folio:health-toast-dismissed:";

/**
 * Tranche B of recovery-visibility: a one-time `toast.info` fired on
 * reader open when the current issue's file has open health-issues.
 * The toast surfaces what's known to be wrong (or what was repaired)
 * *before* the user notices missing pages mid-read.
 *
 * Dismissal is per-issue, stored in `localStorage`. Once a user has
 * acknowledged the warning for an issue, opening that issue again
 * stays silent — the badge on the detail page remains the persistent
 * indicator. Re-opening the issue from a different device or after
 * clearing site data re-triggers the toast.
 *
 * Renders nothing. Effect-only.
 */
export function ReaderHealthToast({
  seriesSlug,
  issueSlug,
}: {
  seriesSlug: string;
  issueSlug: string;
}) {
  const { data } = useIssueHealth(seriesSlug, issueSlug);
  // Once-per-mount guard so a render loop can't fire the toast
  // repeatedly. The localStorage check handles cross-mount dedup.
  const fired = useRef(false);

  useEffect(() => {
    if (fired.current) return;
    if (!data || data.length === 0) return;
    const storageKey = `${STORAGE_PREFIX}${seriesSlug}/${issueSlug}`;
    if (typeof window === "undefined") return;
    try {
      if (window.localStorage.getItem(storageKey) === "1") return;
    } catch {
      // Private mode / blocked storage — surface the toast anyway.
    }

    const { message, severity } = describe(data);
    fired.current = true;
    const handler = severity === "warning" ? toast.warning : toast.info;
    handler(message, {
      duration: 8_000,
      action: {
        label: "Got it",
        onClick: () => {
          try {
            window.localStorage.setItem(storageKey, "1");
          } catch {
            // Storage failed — user will see the toast again next visit.
          }
        },
      },
    });
  }, [data, seriesSlug, issueSlug]);

  return null;
}

function describe(rows: HealthIssueView[]): {
  message: string;
  severity: "warning" | "info";
} {
  const warnings = rows.filter((r) => r.severity === "warning");
  if (warnings.length > 0) {
    const first = warnings[0]!;
    const parts: string[] = ["This file is partial."];
    if (first.kind === "SkippedArchiveEntries") {
      const d = payloadData(first.payload);
      const dropped = numberFromPayload(d?.dropped);
      const total = numberFromPayload(d?.total);
      if (dropped !== null && total !== null) {
        parts.push(`${dropped} of ${total} pages were rejected during scan.`);
      }
      const reason = stringFromPayload(d?.reason);
      if (reason) parts.push(`Reason: ${reason}.`);
    }
    return { message: parts.join(" "), severity: "warning" };
  }
  // Info-only — recovered archive(s).
  const first = rows[0]!;
  const d = payloadData(first.payload);
  const technique = stringFromPayload(d?.technique) ?? "unknown";
  return {
    message: `This file was repaired during scan via ${technique}. Reading should work normally.`,
    severity: "info",
  };
}

function payloadData(payload: unknown): Record<string, unknown> | undefined {
  if (!payload || typeof payload !== "object") return undefined;
  const data = (payload as Record<string, unknown>).data;
  return data && typeof data === "object"
    ? (data as Record<string, unknown>)
    : undefined;
}

function numberFromPayload(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function stringFromPayload(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}
