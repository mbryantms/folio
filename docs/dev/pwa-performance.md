# PWA performance results

## Baseline from review

The pre-change local build had 254 JS/CSS precache candidates: approximately
9.0 MiB raw and 2.7 MiB gzip. These are local artifact sizes, not measured
network transfers. The new precache is restricted to `offline.html`; Next
assets enter the explicit static runtime cache only when requested.

## Measurement log

No physical-device baseline has been recorded yet. Populate the following
fields using the protocol in `pwa-hardening.md`, and retain raw results with
the PR. Do not replace missing measurements with desktop estimates.

| Build/device/network        | Cold/warm usable library | First readable page | Page turn median/p95 | Transfer bytes | Worker install bytes/time | Retained pixels             |
| --------------------------- | ------------------------ | ------------------- | -------------------- | -------------- | ------------------------- | --------------------------- |
| Pending physical-device run | Not measured             | Not measured        | Not measured         | Not measured   | Not measured              | Budget: 24M prefetch pixels |

## Local verification, 2026-09-18

Production build, local Next server without the Rust backend, headless Chromium.
These single-run smoke observations are **not** an installed-device baseline:

| Profile                    | Worker ready after registration request | Offline document navigation to visible heading |
| -------------------------- | --------------------------------------- | ---------------------------------------------- |
| Desktop Chromium           | 66 ms                                   | 34 ms                                          |
| Pixel 7 Chromium emulation | 54 ms                                   | 44 ms                                          |

The compiled precache contains one document, 1.52 kB. Reader first-load JS is
approximately 190.5 KB gzip across 20 chunks, passing the repository's current 195 KB gate
but still above its 150 KB target. Backend-dependent reader latency and
physical-device memory remain unmeasured. Raw JSON attachments are emitted in
`web/test-results/results.json` and CI retains that directory, including offline
screenshots and failure traces.
