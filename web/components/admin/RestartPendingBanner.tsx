"use client";

import { RotateCw } from "lucide-react";

import { useRestartPending } from "@/lib/api/queries";
import { statusTone } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

/** Friendly labels for the boot-only setting keys the endpoint can return.
 *  Mirrors the card labels on /admin/server + /admin/metadata so the banner
 *  reads the same as the form that produced the change. Unknown keys fall
 *  back to the raw registry key. */
const KEY_LABELS: Record<string, string> = {
  "cache.zip_lru_capacity": "ZIP LRU capacity",
  "workers.scan_count": "Scan workers",
  "workers.post_scan_count": "Post-scan workers",
  "workers.scan_batch_size": "Scan batch size",
  "workers.scan_hash_buffer_kb": "Hash buffer (KB)",
  "workers.archive_work_parallel": "Archive work parallel",
  "workers.thumb_inline_parallel": "Thumb inline parallel",
  "metadata.weekly_refresh_cron": "Metadata refresh schedule",
};

/**
 * Surfaces boot-only settings (worker pools, ZIP LRU, the metadata
 * weekly-refresh cron) that have been changed since the server started and
 * so won't take effect until it restarts. Driven by a boot-snapshot-vs-live
 * `Config` diff on the server (`GET /admin/server/restart-pending`).
 *
 * Self-hides when nothing is pending. Mounted only inside the admin shell
 * (the query is admin-only), so it never fires on the shared settings tree.
 */
export function RestartPendingBanner() {
  const { data } = useRestartPending();
  const pending = data?.pending ?? [];
  if (pending.length === 0) return null;

  return (
    <div
      role="status"
      className={cn(
        "mb-6 flex items-start gap-3 rounded-lg border p-4 text-sm",
        statusTone("warning"),
      )}
    >
      <RotateCw className="mt-0.5 h-4 w-4 shrink-0" />
      <div className="min-w-0 space-y-1.5">
        <p className="font-medium">
          {pending.length === 1
            ? "1 setting needs a restart to take effect"
            : `${pending.length} settings need a restart to take effect`}
        </p>
        <ul className="space-y-0.5">
          {pending.map((p) => (
            <li key={p.key} className="text-xs wrap-anywhere">
              <span className="font-medium">{KEY_LABELS[p.key] ?? p.key}</span>:{" "}
              <span className="font-mono">{p.boot_value}</span>
              {" → "}
              <span className="font-mono">{p.current_value}</span>
            </li>
          ))}
        </ul>
        <p className="text-xs opacity-80">
          The new values are saved and will load the next time the server
          process restarts.
        </p>
      </div>
    </div>
  );
}
