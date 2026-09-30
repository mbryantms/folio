import { describe, expect, it } from "vitest";

import { describeLastTrigger } from "@/components/admin/library/WatchersCard";
import type { WatcherStatus } from "@/lib/api/types";

const NOW = Date.parse("2026-09-29T12:00:00Z");

function status(over: Partial<WatcherStatus>): WatcherStatus {
  return {
    mode: "inotify",
    filesystem: "ext4",
    started_at: "2026-09-29T11:00:00Z",
    last_event_at: null,
    last_trigger_at: null,
    last_trigger_dirs: 0,
    last_scan_id: null,
    last_trigger_coalesced: false,
    triggers_total: 0,
    detail: null,
    ...over,
  };
}

describe("describeLastTrigger", () => {
  it("shows a dash for a disabled watcher that never fired", () => {
    expect(describeLastTrigger(status({ mode: "disabled" }), NOW)).toBe("—");
  });

  it("says nothing has happened yet for a live watcher", () => {
    expect(describeLastTrigger(status({ mode: "poll" }), NOW)).toBe(
      "No changes seen yet",
    );
  });

  it("describes a scoped trigger", () => {
    expect(
      describeLastTrigger(
        status({
          last_trigger_at: "2026-09-29T11:55:00Z",
          last_trigger_dirs: 3,
        }),
        NOW,
      ),
    ).toBe("5 mins ago · 3 folders");
  });

  it("flags overflow rescans and coalesced triggers", () => {
    expect(
      describeLastTrigger(
        status({
          last_trigger_at: "2026-09-29T11:59:30Z",
          last_trigger_dirs: 0,
          last_trigger_coalesced: true,
        }),
        NOW,
      ),
    ).toBe("just now · full rescan (event overflow) · joined running scan");
  });
});
