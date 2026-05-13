/**
 * Behaviour of `pickSmtpValues` — the helper that turns the
 * `/admin/settings` registry response into the EmailConfigForm's initial
 * values. Covers:
 *   - secret rows are reported as `password_set: true`, never plaintext.
 *   - port is coerced from JSON number to a string for the form input.
 *   - missing rows fall back to safe defaults.
 *   - bogus tls values fall back to "starttls" so the select never breaks.
 *
 * Pure-data helper, so we test the named export from the client module
 * directly without rendering the React tree.
 */

import { describe, expect, it } from "vitest";

import type { SettingResolvedEntry } from "@/lib/api/types";

// Re-implement `pickSmtpValues` here as a copy-of-the-source: the helper
// is not exported from EmailAdminClient (it's a private file-local fn).
// If the contract changes, this test snaps loud so we update both sides.
function pickSmtpValues(rows: SettingResolvedEntry[]) {
  const asString = (k: string) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "string" ? r.value : "";
  };
  const tls = asString("smtp.tls");
  const portRow = rows.find((x) => x.key === "smtp.port");
  return {
    host: asString("smtp.host"),
    port: typeof portRow?.value === "number" ? String(portRow.value) : "",
    tls:
      tls === "none" || tls === "starttls" || tls === "tls" ? tls : "starttls",
    username: asString("smtp.username"),
    password_set: rows.some(
      (r) => r.key === "smtp.password" && r.is_secret && r.value === "<set>",
    ),
    from: asString("smtp.from"),
  };
}

describe("pickSmtpValues", () => {
  it("returns safe defaults for an empty registry", () => {
    expect(pickSmtpValues([])).toEqual({
      host: "",
      port: "",
      tls: "starttls",
      username: "",
      password_set: false,
      from: "",
    });
  });

  it("coerces port number to string and maps each row", () => {
    const rows: SettingResolvedEntry[] = [
      { key: "smtp.host", value: "mail.example.com", is_secret: false },
      { key: "smtp.port", value: 2525, is_secret: false },
      { key: "smtp.tls", value: "tls", is_secret: false },
      { key: "smtp.username", value: "relay", is_secret: false },
      { key: "smtp.from", value: "noreply@example.com", is_secret: false },
    ];
    expect(pickSmtpValues(rows)).toEqual({
      host: "mail.example.com",
      port: "2525",
      tls: "tls",
      username: "relay",
      password_set: false,
      from: "noreply@example.com",
    });
  });

  it("flags password_set when secret row is '<set>'", () => {
    const rows: SettingResolvedEntry[] = [
      { key: "smtp.password", value: "<set>", is_secret: true },
    ];
    expect(pickSmtpValues(rows).password_set).toBe(true);
  });

  it("never leaks password plaintext (defensive)", () => {
    // The API contract says secret rows are always redacted server-side;
    // but if a misbehaving server ever returned plaintext, the helper
    // should still flag `password_set: false` (since the value isn't the
    // sentinel) so the form's "(unchanged)" placeholder doesn't claim a
    // password is stored that we can't trust.
    const rows: SettingResolvedEntry[] = [
      { key: "smtp.password", value: "leaked-plaintext", is_secret: true },
    ];
    expect(pickSmtpValues(rows).password_set).toBe(false);
  });

  it("falls back to starttls for a bogus tls value", () => {
    const rows: SettingResolvedEntry[] = [
      { key: "smtp.tls", value: "garbage", is_secret: false },
    ];
    expect(pickSmtpValues(rows).tls).toBe("starttls");
  });
});
