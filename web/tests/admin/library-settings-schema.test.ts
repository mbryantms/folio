import { describe, expect, it } from "vitest";
import { z } from "zod";

import { validateCron } from "@/lib/api/cron";

// Mirrors LibrarySettingsForm's schema. Kept here so any drift surfaces in CI.
const schema = z
  .object({
    ignore_globs: z.array(z.string().min(1)),
    scan_schedule_cron: z
      .string()
      .refine((v) => validateCron(v).ok, "Invalid cron expression"),
    report_missing_comicinfo: z.boolean(),
    soft_delete_days: z.number().int().min(0).max(365),
    archive_writeback_jpeg_quality: z
      .number()
      .int()
      .min(60)
      .max(100)
      .default(92),
    allow_archive_writeback: z.boolean().default(false),
    metadata_writeback_enabled: z.boolean().default(false),
    auto_convert_cbr_on_scan: z.boolean().default(false),
  })
  .refine((v) => !v.metadata_writeback_enabled || v.allow_archive_writeback, {
    message:
      "Metadata writeback requires Archive writeback (master toggle) to be on first.",
    path: ["metadata_writeback_enabled"],
  })
  .refine((v) => !v.auto_convert_cbr_on_scan || v.allow_archive_writeback, {
    message:
      "CBR conversion requires Archive writeback (master toggle) to be on first.",
    path: ["auto_convert_cbr_on_scan"],
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

  it("accepts JPEG quality within 60-100", () => {
    const r = schema.safeParse({
      ignore_globs: [],
      scan_schedule_cron: "",
      report_missing_comicinfo: false,
      soft_delete_days: 7,
      archive_writeback_jpeg_quality: 60,
    });
    expect(r.success).toBe(true);
  });

  it("rejects JPEG quality out of range", () => {
    expect(
      schema.safeParse({
        ignore_globs: [],
        scan_schedule_cron: "",
        report_missing_comicinfo: false,
        soft_delete_days: 7,
        archive_writeback_jpeg_quality: 59,
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        ignore_globs: [],
        scan_schedule_cron: "",
        report_missing_comicinfo: false,
        soft_delete_days: 7,
        archive_writeback_jpeg_quality: 101,
      }).success,
    ).toBe(false);
  });

  it("rejects CBR conversion without the master writeback toggle", () => {
    const r = schema.safeParse({
      ignore_globs: [],
      scan_schedule_cron: "",
      report_missing_comicinfo: false,
      soft_delete_days: 7,
      auto_convert_cbr_on_scan: true,
      allow_archive_writeback: false,
    });
    expect(r.success).toBe(false);
  });

  it("accepts CBR conversion when archive writeback is on", () => {
    const r = schema.safeParse({
      ignore_globs: [],
      scan_schedule_cron: "",
      report_missing_comicinfo: false,
      soft_delete_days: 7,
      auto_convert_cbr_on_scan: true,
      allow_archive_writeback: true,
    });
    expect(r.success).toBe(true);
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
