import { describe, expect, it } from "vitest";
import { z } from "zod";

import { validateCron } from "@/lib/api/cron";

// Mirrors LibrarySettingsForm's schema. Kept here so any drift surfaces in CI.
const schema = z.object({
  ignore_globs: z.array(z.string().min(1)),
  scan_schedule_cron: z
    .string()
    .refine((v) => validateCron(v).ok, "Invalid cron expression"),
  report_missing_comicinfo: z.boolean(),
  soft_delete_days: z.number().int().min(0).max(365),
});

describe("library settings schema", () => {
  it("accepts a minimal valid payload", () => {
    const r = schema.safeParse({
      ignore_globs: [],
      scan_schedule_cron: "",
      report_missing_comicinfo: false,
      soft_delete_days: 7,
    });
    expect(r.success).toBe(true);
  });

  it("accepts a valid cron + tag list", () => {
    const r = schema.safeParse({
      ignore_globs: ["**/*.tmp", ".trash/**"],
      scan_schedule_cron: "0 */6 * * *",
      report_missing_comicinfo: true,
      soft_delete_days: 30,
    });
    expect(r.success).toBe(true);
  });

  it("rejects an invalid cron", () => {
    const r = schema.safeParse({
      ignore_globs: [],
      scan_schedule_cron: "bogus cron",
      report_missing_comicinfo: false,
      soft_delete_days: 7,
    });
    expect(r.success).toBe(false);
  });

  it("rejects an empty glob entry", () => {
    const r = schema.safeParse({
      ignore_globs: [""],
      scan_schedule_cron: "",
      report_missing_comicinfo: false,
      soft_delete_days: 7,
    });
    expect(r.success).toBe(false);
  });

  it("rejects soft_delete_days out of range", () => {
    expect(
      schema.safeParse({
        ignore_globs: [],
        scan_schedule_cron: "",
        report_missing_comicinfo: false,
        soft_delete_days: -1,
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        ignore_globs: [],
        scan_schedule_cron: "",
        report_missing_comicinfo: false,
        soft_delete_days: 366,
      }).success,
    ).toBe(false);
  });
});
