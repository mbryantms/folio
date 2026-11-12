//! The short Markdown report written next to the JSON.

use std::fmt::Write as _;

use crate::stats::Summary;
use crate::{Agg, Grouped, Results};

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

fn pct(x: f64) -> String {
    if x.is_finite() {
        format!("{:.1}%", x * 100.0)
    } else {
        "–".into()
    }
}

fn warm(s: &Summary) -> (String, String, String) {
    if s.n == 0 {
        ("–".into(), "–".into(), "0".into())
    } else {
        (fmt_ms(s.median_ms), fmt_ms(s.p90_ms), s.n.to_string())
    }
}

fn esc(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " / ")
}

fn agg_header(out: &mut String, first: &str) {
    let _ = writeln!(
        out,
        "| {first} | refs | CER<=5% | <=10% | <=25% | >25% | missed | mean CER | median CER | mean WER | median WER | mean CER, missed=1 |"
    );
    let _ = writeln!(
        out,
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    );
}

fn agg_row(out: &mut String, name: &str, a: &Agg) {
    let _ = writeln!(
        out,
        "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
        esc(name),
        a.n,
        a.le5,
        a.le10,
        a.le25,
        a.over25,
        a.missed,
        pct(a.mean_cer),
        pct(a.median_cer),
        pct(a.mean_wer),
        pct(a.median_wer),
        pct(a.mean_cer_all)
    );
}

fn grouped_table(out: &mut String, g: &Grouped) {
    agg_header(out, "group");
    agg_row(out, "all", &g.overall);
    for (k, a) in &g.by_kind {
        agg_row(out, &format!("kind: {k}"), a);
    }
    for (k, a) in &g.by_colour_case {
        agg_row(out, &format!("colour_case: {k}"), a);
    }
    out.push('\n');
}

pub(crate) fn markdown(r: &Results) -> String {
    let mut o = String::new();
    let e = &r.env;
    let _ = writeln!(o, "# OCR bench: {}\n", r.label);
    let truth_pages = r.pages.iter().filter(|p| p.refs.is_some()).count();
    let _ = writeln!(
        o,
        "- Pages: {} ({} with a reference transcription)\n- Build profile: **{}**\n- CPU: {}; CPUs allowed: `{}` (available_parallelism {}); OMP_NUM_THREADS: `{}`\n- Threads in the process after the first detection: {}\n- Load average (1/5/15 min) at start: `{}`; at end: `{}`\n- Tesseract {}; tessdata `{}`\n- Detector: {}\n- Recogniser: {}\n",
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
        e.tesseract_version,
        e.tessdata,
        r.detector,
        r.recognizer
    );
    for n in &r.notes {
        let _ = writeln!(o, "- Note: {n}");
    }

    if !r.timing_rows.is_empty() {
        let _ = writeln!(
            o,
            "\n## Timings (wall clock)\n\nCold = first run in the process (first page, first region). Warm = repeats; median and 90th percentile over all warm samples.\n"
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

    for s in &r.e2e {
        let _ = writeln!(
            o,
            "\n## End to end: detect, then recognise ({})\n\nProduced regions: {}; matching no reference: {} with text, {} empty.\n",
            s.set, s.produced_total, s.unmatched_nonempty, s.unmatched_empty
        );
        grouped_table(&mut o, &s.refs);
    }

    if !r.margins.is_empty() {
        let _ = writeln!(
            o,
            "\n## Recognition alone (reference box + margin, `detect: false`, Folio postprocess)\n"
        );
        for m in &r.margins {
            let (tm, tp, _) = warm(&m.time);
            let _ = writeln!(
                o,
                "### Margin {:.0}% of the box\n\nTime per region (crop already made): median {tm}, p90 {tp}. Without postprocess: mean CER {}, mean WER {}.\n",
                m.margin * 100.0,
                pct(m.raw.mean_cer),
                pct(m.raw.mean_wer)
            );
            grouped_table(&mut o, &m.clean);
        }
    }

    if !r.variants.is_empty() {
        let _ = writeln!(
            o,
            "\n## Tesseract variants (one change each against `current`, same crops, margin {:.0}%)\n\nThe bench's `current` reproduces the server recogniser's raw text on {} of {} crops.\n\n| variant | change | mean CER | median CER | mean WER | CER<=10% | mean CER raw (no postprocess) | mean WER raw | median time | p90 time | note |\n|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|",
            r.args.variant_margin * 100.0,
            r.current_matches_server.0,
            r.current_matches_server.1
        );
        for v in &r.variants {
            let (tm, tp, _) = warm(&v.time);
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {}/{} | {} | {} | {} | {} | {} |",
                v.name,
                v.what_changed,
                pct(v.clean.mean_cer),
                pct(v.clean.median_cer),
                pct(v.clean.mean_wer),
                v.clean.le10,
                v.clean.n,
                pct(v.raw.mean_cer),
                pct(v.raw.mean_wer),
                tm,
                tp,
                esc(v.error.as_deref().unwrap_or(""))
            );
        }
    }

    if !r.worst.is_empty() {
        let _ = writeln!(
            o,
            "\n## Worst {} reference regions (recognition alone, margin {:.0}%, current pipeline)\n\n| # | page | id | kind | colour | CER | reference | produced | crop |\n|---:|---|---|---|---|---:|---|---|---|",
            r.worst.len(),
            r.args.variant_margin * 100.0
        );
        for w in &r.worst {
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                w.rank,
                w.page,
                esc(&w.ref_id),
                esc(&w.kind),
                esc(&w.colour_case),
                pct(w.cer),
                esc(&w.reference),
                esc(&w.produced),
                w.crop_image
            );
        }
    }
    o
}
