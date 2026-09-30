# Metrics (`/metrics`)

Folio exposes Prometheus metrics in the text exposition format at **`GET /metrics`**
on the Rust origin (e.g. `http://localhost:8080/metrics`). Every series carries a
`service="folio"` label; all metric names use the `folio_` prefix.

The recorder is installed once at boot ([`observability::init`](../../crates/server/src/observability.rs));
the endpoint is served by [`api::meta`](../../crates/server/src/api/meta.rs).

> **Lazy registration:** counters/histograms appear only after the relevant code
> path runs at least once. Boot-time gauges (`folio_process_*`,
> `folio_jobs_queue_depth`, `folio_metadata_writeback_libraries_remaining`,
> `folio_thumbs_bytes`) show immediately (`folio_thumbs_bytes` once the boot
> walk of `thumbs/` finishes).

## Catalogue

### HTTP (RED) — emitted by [`middleware::http_metrics`](../../crates/server/src/middleware/http_metrics.rs)
| Metric | Type | Labels |
|---|---|---|
| `folio_http_requests_total` | counter | `method`, `route`, `status` |
| `folio_http_request_duration_seconds` | histogram | `method`, `route` |

`route` is the matched route **pattern** (e.g. `/series/{series_slug}/issues/{issue_slug}`),
never the raw URI — unmatched/proxied requests bucket under `"<unmatched>"`. The
`/metrics` scrape itself is not counted.

### Process / runtime — `metrics-process` collector, sampled per scrape
`folio_process_cpu_seconds_total`, `folio_process_resident_memory_bytes`,
`folio_process_virtual_memory_bytes`, `folio_process_open_fds`,
`folio_process_max_fds`, `folio_process_threads`, `folio_process_start_time_seconds`.

### Jobs (apalis workers) — [`jobs::metrics_layer`](../../crates/server/src/jobs/metrics_layer.rs) + scheduler
| Metric | Type | Labels |
|---|---|---|
| `folio_jobs_processed_total` | counter | `kind`, `status` (`success`\|`failed`) |
| `folio_job_duration_seconds` | histogram | `kind` |
| `folio_jobs_queue_depth` | gauge | `queue` |

`kind`/`queue` ∈ `scan`, `scan_series`, `post_scan_thumbs`, `post_scan_search`,
`post_scan_dictionary`, `metadata_search_series`, `metadata_search_issue`,
`metadata_apply_series`, `metadata_apply_issue`, `rewrite_issue_sidecars`,
`archive_edit`. Queue-depth refreshes every 30s ([`jobs::scheduler`](../../crates/server/src/jobs/scheduler.rs)).

### Thumbnail storage — [`library::thumbnails`](../../crates/server/src/library/thumbnails.rs) (WP-3.8, audit OP-5)
| Metric | Type | Labels |
|---|---|---|
| `folio_thumbs_bytes` | gauge | — |
| `folio_thumbs_evicted_files_total` | counter | — |

`folio_thumbs_bytes` is the total size of `data_path/thumbs/` — generated covers
(`{id}.webp`, `{id}@sm.webp`), page strips (`{id}/s/*`) and downloaded provider
covers (`issues/{id}/covers/*`). Refreshed by the thumbnail budget sweep
([`jobs::orphan_sweep::run_budget_sweep`](../../crates/server/src/jobs/orphan_sweep.rs)):
once at boot, hourly at `:17`, after the daily 04:30 UTC orphan sweep, and
(throttled to one per 5 min, only when a budget is set) after thumbnail writes.
The eviction counter only moves when the optional `cache.thumbs_budget_mb`
budget is set — see [runtime-configuration.md](runtime-configuration.md).
Alert idea: `folio_thumbs_bytes` growing toward the data volume's free space.

### Subsystem counters (pre-existing)
`folio_scan_duration_seconds`, `folio_scan_files_total`, `folio_scan_health_issues_open`,
`folio_zip_lru_{hits,misses,evictions}_total`, `folio_zip_lru_open_fds`,
`folio_ocr_{pipeline,recognize}_seconds`, `folio_ocr_{cache,detect_cache}_{hits,misses}_total`,
`folio_auth_lockout_total`, `folio_auth_lockout_email_total`,
`folio_rate_limit_denied_total`, `folio_csp_violations_total`,
`folio_metadata_writeback_libraries_remaining`.

Histograms named `*_seconds` use buckets
`0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10`.

## Scraping

```yaml
# prometheus.yml
scrape_configs:
  - job_name: folio
    metrics_path: /metrics
    static_configs:
      - targets: ['folio-host:8080']
    authorization:
      type: Bearer
      credentials: "<COMIC_METRICS_TOKEN>"
```

## Security

`/metrics` uses machine bearer auth in production/release builds. Set
`COMIC_METRICS_TOKEN` and configure Prometheus with `Authorization: Bearer ...`.
A debug/test build remains open by default for local development.

If you intentionally expose unauthenticated metrics in production, set
`COMIC_METRICS_OPEN=true` and protect the route with a reverse-proxy/network ACL.
Metric values leak operational signal such as request counts, latencies, and
lockout counts.
