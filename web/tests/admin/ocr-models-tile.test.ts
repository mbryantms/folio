/**
 * Unit tests for the OCR-models tile state classifier
 * (text-detection-1.0 plan, M7).
 *
 * The contract this pins:
 *   - `missing` when nothing's on disk yet (operator hasn't OCRed).
 *   - `downloading` once first byte lands but before the rough size
 *     target — the badge then reads `"downloading N%"`.
 *   - `ready` once we're within 5% of the expected size. We accept
 *     overshoot because `expected_bytes_approx` is a hand-picked
 *     round number, not a sha-pinned manifest.
 *
 * Verifying this in isolation means the dashboard can't silently
 * regress to a binary "missing | present" UI — operators waiting on
 * a slow HF download depend on the intermediate `downloading` state
 * for feedback.
 */
import { describe, expect, it } from "vitest";

import {
  classifyModelState,
  formatBytes,
} from "@/components/admin/observability/ServerInfoClient";
import type { OcrModelView } from "@/lib/api/types";

function model(overrides: Partial<OcrModelView> = {}): OcrModelView {
  return {
    id: "comic-text-detector",
    purpose: "Text-bubble detection",
    kind: "onnx",
    cache_dir: "/cache/hf/hub/models--mayocream--comic-text-detector-onnx",
    present: true,
    bytes_on_disk: 95 * 1024 * 1024,
    expected_bytes_approx: 95 * 1024 * 1024,
    source: "huggingface.co/mayocream/comic-text-detector-onnx",
    ...overrides,
  };
}

describe("classifyModelState", () => {
  it("returns `missing` when nothing is on disk", () => {
    const m = model({ present: false, bytes_on_disk: 0 });
    expect(classifyModelState(m)).toEqual({
      kind: "missing",
      pct: 0,
      label: "missing",
    });
  });

  it("returns `missing` even if the present flag drifted from bytes_on_disk", () => {
    // Defensive: if a future server bug set `present` without
    // populating bytes, we shouldn't render a misleading
    // "downloading 0%". `bytes_on_disk == 0 → missing`, full stop.
    const m = model({ present: true, bytes_on_disk: 0 });
    expect(classifyModelState(m).kind).toBe("missing");
  });

  it("returns `downloading` between first byte and the 95% threshold", () => {
    const m = model({
      present: true,
      bytes_on_disk: 40 * 1024 * 1024,
      expected_bytes_approx: 95 * 1024 * 1024,
    });
    const state = classifyModelState(m);
    expect(state.kind).toBe("downloading");
    expect(state.pct).toBe(42);
    expect(state.label).toBe("downloading 42%");
  });

  it("returns `ready` at 95% and above", () => {
    const m = model({
      bytes_on_disk: Math.round(0.95 * 95 * 1024 * 1024),
      expected_bytes_approx: 95 * 1024 * 1024,
    });
    expect(classifyModelState(m).kind).toBe("ready");
  });

  it("clamps pct at 100 when bytes_on_disk overshoots expected", () => {
    // `expected_bytes_approx` is a hand-picked round number; the HF
    // blob can be slightly larger. The badge mustn't read `103%`.
    const m = model({
      bytes_on_disk: 100 * 1024 * 1024,
      expected_bytes_approx: 95 * 1024 * 1024,
    });
    const state = classifyModelState(m);
    expect(state.pct).toBe(100);
    expect(state.kind).toBe("ready");
  });

  it("treats expected=0 as ready (avoids divide-by-zero pct)", () => {
    // Shouldn't happen with the current model registry, but the
    // helper has to handle it without producing NaN.
    const m = model({ expected_bytes_approx: 0, bytes_on_disk: 1024 });
    expect(classifyModelState(m).kind).toBe("ready");
    expect(classifyModelState(m).pct).toBe(100);
  });
});

describe("formatBytes", () => {
  it("returns `0 B` for zero", () => {
    expect(formatBytes(0)).toBe("0 B");
  });

  it("renders bytes verbatim under 1 kB", () => {
    expect(formatBytes(512)).toBe("512 B");
  });

  it("picks larger units with one-decimal precision under 10", () => {
    expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });

  it("rounds whole numbers when the value is 10 or higher", () => {
    expect(formatBytes(95 * 1024 * 1024)).toBe("95 MB");
    expect(formatBytes(2 * 1024 * 1024 * 1024)).toBe("2.0 GB");
  });
});
