import { describe, expect, it } from "vitest";

import { issueStateHint, issueStateLabel } from "@/lib/library/issue-state";

describe("issue parse-state copy", () => {
  it("labels an encrypted issue as password-protected with a why", () => {
    expect(issueStateLabel("encrypted")).toBe("Password-protected");
    expect(issueStateHint("encrypted")).toMatch(/can't read encrypted/);
  });

  it("covers the other scanner parse state", () => {
    expect(issueStateLabel("malformed")).toBe("Unreadable archive");
    expect(issueStateHint("malformed")).toBeTruthy();
  });

  it("falls back to the raw token for an unknown state", () => {
    expect(issueStateLabel("quarantined")).toBe("quarantined");
    expect(issueStateHint("quarantined")).toBeUndefined();
  });
});
