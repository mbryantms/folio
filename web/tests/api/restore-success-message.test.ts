/**
 * The archive restore's success toast lives on the hook
 * (`useRestoreArchiveMutation`, via `restoreSuccessMessage`): it says the
 * archive was restored, carries the server's warning or rebuild sentence
 * when there is one, and says a restore past the server's deadline is
 * still running.
 */
import { describe, expect, it } from "vitest";

import { restoreSuccessMessage } from "@/lib/api/mutations";
import type { RestoreResponse } from "@/lib/api/types";

const base: RestoreResponse = {
  issue_id: "i1",
  status: "restored",
  in_progress: false,
  kept_as: "/lib/x.cbz.bak",
  warning: null,
  rebuilt_from: null,
  message:
    "The backup was restored; the replaced archive is kept as x.cbz.bak.",
};

describe("restoreSuccessMessage", () => {
  it("says the archive was restored", () => {
    expect(restoreSuccessMessage(base)).toBe("Archive restored");
  });

  it("carries the warning when the replaced archive kept another name", () => {
    const msg = restoreSuccessMessage({
      ...base,
      warning: "kept as x.cbz.bak.ab.tmp",
    });
    expect(msg).toBe("Archive restored. kept as x.cbz.bak.ab.tmp");
  });

  it("uses the server's sentence for a rebuild from a pre-conversion backup", () => {
    const message =
      "The archive was rebuilt as a CBZ from the pre-conversion backup x.cbr.bak (the original bytes stay available in that backup).";
    expect(
      restoreSuccessMessage({ ...base, rebuilt_from: "x.cbr.bak", message }),
    ).toBe(message);
  });

  it("says a restore past the deadline is still running", () => {
    const msg = restoreSuccessMessage({
      ...base,
      status: "in_progress",
      in_progress: true,
      kept_as: null,
    });
    expect(msg).toMatch(/still running/);
  });
});
