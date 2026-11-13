//! The short Markdown report written next to the JSON.

use std::fmt::Write as _;

use crate::Results;
use crate::stats::Summary;

fn ms(v: Option<f64>) -> String {
    match v {
        Some(x) if x.is_finite() => fmt_ms(x),
        _ => "–".into(),
    }
}

fn fmt_ms(x: f64) -> String {
    if x >= 1000.0 {
        format!("{:.2} s", x / 1000.0)
    } else if x >= 10.0 {
        format!("{x:.0} ms")
    } else {
        format!("{x:.2} ms")
    }
}

fn warm(s: &Summary) -> (String, String, String) {
    if s.n == 0 {
        ("–".into(), "–".into(), "0".into())
    } else {
        (fmt_ms(s.median_ms), fmt_ms(s.p90_ms), s.n.to_string())
    }
}

pub(crate) fn markdown(r: &Results) -> String {
    let mut o = String::new();
    let e = &r.env;
    let _ = writeln!(o, "# OCR bench: {}\n", r.label);
    let truth_pages = r.pages.iter().filter(|p| p.refs.is_some()).count();
    let _ = writeln!(
        o,
        "- Pages: {} ({} with a reference transcription)\n- Build profile: **{}**\n- CPU: {}; CPUs allowed: `{}` (available_parallelism {}); OMP_NUM_THREADS: `{}`\n- Threads in the process after the first detection: {}\n- Load average (1/5/15 min) at start: `{}`; at end: `{}`\n- Detector: {}\n",
        r.pages.len(),
        truth_pages,
        e.profile,
        e.cpu_model,
        e.cpus_allowed,
        e.available_parallelism,
        e.omp_num_threads.as_deref().unwrap_or("(unset)"),
        e.threads_after_detect
            .map(|t| t.to_string())
            .unwrap_or_else(|| "–".into()),
        e.loadavg_start,
        e.loadavg_end,
        r.detector
    );

    if !r.timing_rows.is_empty() {
        let _ = writeln!(
            o,
            "\n## Timings (wall clock)\n\nCold = first run in the process (first page). Warm = repeats; median and 90th percentile over all warm samples.\n"
        );
        let _ = writeln!(o, "| stage | input | cold | warm median | warm p90 | n |");
        let _ = writeln!(o, "|---|---|---:|---:|---:|---:|");
        for row in &r.timing_rows {
            let (m, p, n) = warm(&row.warm);
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} |",
                row.stage,
                row.input,
                ms(row.cold_ms),
                m,
                p,
                n
            );
        }
        if !r.composed.is_empty() {
            let _ = writeln!(
                o,
                "\n### User-visible latencies (composed from per-page warm medians)\n\n| latency | definition | cold | median | p90 | n |\n|---|---|---:|---:|---:|---:|"
            );
            for c in &r.composed {
                let (m, p, n) = warm(&c.warm);
                let _ = writeln!(
                    o,
                    "| {} | {} | {} | {} | {} | {} |",
                    c.name,
                    c.definition,
                    ms(c.cold_ms),
                    m,
                    p,
                    n
                );
            }
        }
    }

    if r.pages
        .iter()
        .any(|p| !p.boxes.is_empty() || p.sheet.is_some())
    {
        let _ = writeln!(
            o,
            "\n## Detection per page\n\nContact sheets: red = class 0, blue = class 1, green = reference box. Nested = at least 80% inside a larger box (the reader panel drops these).\n\n| page | size | class 0 | class 1 | nested | refs | refs with no box overlapping | refs with no box covering half | sheet |\n|---|---|---:|---:|---:|---:|---:|---:|---|"
        );
        for p in &r.pages {
            let opt = |v: Option<usize>| v.map(|x| x.to_string()).unwrap_or_else(|| "–".into());
            let _ = writeln!(
                o,
                "| {} | {}x{} | {} | {} | {} | {} | {} | {} | {} |",
                p.key,
                p.width,
                p.height,
                p.class_counts[0],
                p.class_counts[1],
                p.nested_count,
                opt(p.refs),
                opt(p.refs_without_overlap),
                opt(p.refs_without_half_cover),
                p.sheet.as_deref().unwrap_or("–")
            );
        }
    }

    o
}
