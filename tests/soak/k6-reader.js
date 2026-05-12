/**
 * k6 reader soak (§16.5) — End-of-Phase 2.
 *
 * Run: `k6 run -e BASE=http://localhost:8080 -e BEARER=<jwt> -e ISSUE=<id> -e PAGES=24 tests/soak/k6-reader.js`
 *
 * 10 virtual users page through the same issue in sequence for 1 hour. Asserts
 * no 5xx responses and bounded p99 latency. Run alongside a `/metrics` watcher
 * to confirm `comic_zip_lru_open_fds` stays ≤ capacity and RSS does not grow.
 *
 * Phase 2 done-when criteria require this script to run cleanly against
 * `compose.prod.yml` before declaring the phase shipped.
 */
import http from "k6/http";
import { check, sleep } from "k6";

const BASE = __ENV.BASE || "http://localhost:8080";
const BEARER = __ENV.BEARER;
const ISSUE = __ENV.ISSUE;
const PAGES = parseInt(__ENV.PAGES || "10", 10);

if (!BEARER) {
  throw new Error("Set BEARER=<jwt> (admin or scoped). See docs/dev/environment.md.");
}
if (!ISSUE) {
  throw new Error("Set ISSUE=<issue_id> — a content-hash for an active issue.");
}

export const options = {
  scenarios: {
    page_through: {
      executor: "constant-vus",
      vus: 10,
      duration: "1h",
      gracefulStop: "30s",
    },
  },
  thresholds: {
    // No server errors at all — page bytes must always succeed.
    "http_req_failed{type:page}": ["rate==0.0"],
    // Bound the p99 to keep an FD-leak / queue-buildup regression visible.
    "http_req_duration{type:page}": ["p(99)<2000"],
    checks: ["rate==1.00"],
  },
};

export default function () {
  for (let n = 0; n < PAGES; n += 1) {
    const res = http.get(`${BASE}/issues/${ISSUE}/pages/${n}`, {
      headers: { Authorization: `Bearer ${BEARER}` },
      tags: { type: "page" },
    });
    check(res, {
      "200 or 206": (r) => r.status === 200 || r.status === 206,
      "has Content-Type": (r) => !!r.headers["Content-Type"],
    });
    sleep(0.5); // simulate read time between pages
  }
}
