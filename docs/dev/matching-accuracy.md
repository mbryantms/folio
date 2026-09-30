## Matching accuracy

Operator + developer reference for the metadata-provider matching
pipeline. Covers the ComicTagger-derived heuristics shipped in
`matching-accuracy-1.0`, the operator-tunable knobs, the telemetry
table, and the recipe for adding regression-suite fixtures when a
production miss is reported.

### Pipeline at a glance

A search runs in this order:

1. **Pre-filter** ([`orchestrator::pre_filter_series`](../../crates/server/src/metadata/orchestrator.rs))
   drops candidates that the operator's library settings + the hard
   year gate would reject before any scoring. Two signals:
   - Year gate: candidate's `start_year > comic_year + 1` → drop.
     **Escape (WP-2.8):** when the gate leaves zero candidates and a
     local cover hash exists, the orchestrator re-scores the *same*
     provider results under `YearGate::PhashAware` (a mismatched year
     survives only with a MEDIUM-or-better cover Hamming) and stamps
     `year_gate_relaxed` on `metadata_run.query`. Never when the user
     supplied the year as a search override (`SearchOpts::relax_year_gate
     = false`). The issue path applies the same escape to the
     *narrowed* pass before its broad fallback.
   - Publisher blacklist: candidate's publisher (sanitized) matches
     any entry in `library.metadata_publisher_blacklist` → drop.
2. **Score** (text + cover pHash) per surviving candidate. Text
   pipeline:
   - Title sanitization ([`metadata::title_norm::sanitize_title`](../../crates/server/src/metadata/title_norm.rs)):
     NFKD → casefold → quote-strip → punctuation→hyphen → strip 23
     ComicTagger article words.
   - Ratcliff/Obershelp similarity ([`metadata::ratcliff::three_pass_ratio`](../../crates/server/src/metadata/ratcliff.rs)):
     three-pass upper-bound chain mirroring Python's
     `difflib.SequenceMatcher.ratio`.
   - Score components: 0.45 name + 0.20 year + 0.15 publisher +
     0.15 issue_number + 0.05 volume = 100 max.
3. **Cover-pHash override** ([`Score::bucket`](../../crates/server/src/metadata/matcher.rs))
   consults the cover Hamming distance before the text score. The
   ComicTagger-derived ladder (verbatim constants):
   - ≤ 8 bits (`STRONG_SCORE_THRESH`) → HIGH
   - ≤ 16 bits (`MIN_SCORE_THRESH`) → MEDIUM
   - > 16 bits → LOW (cover veto)
   - No phash on either side → fall back to operator text thresholds.

   The local hash is computed on the archive's cover page **after**
   [`thumbnails::front_cover_crop`](../../crates/server/src/library/thumbnails.rs):
   a wraparound (back + front scanned as one landscape image) is cut
   to its front half first, mirroring ComicTagger's
   `IssueIdentifier.crop_cover`. Providers host the front alone, so
   hashing the full spread would park a genuine match 20+ bits away.

   When the winning cover came from a **variant / alternate** slot
   the MEDIUM ceiling tightens to ≤ 12 (`MIN_ALTERNATE_SCORE_THRESH`)
   — a variant match needs to be tighter to qualify since the
   candidate's "real" cover may differ.
4. **Gap-to-next-best guard**
   ([`orchestrator::finalize_ranking`](../../crates/server/src/metadata/orchestrator.rs)):
   when the top two cover-Hamming candidates are within 4 bits
   (`MIN_SCORE_DISTANCE`) AND the winner is HIGH, downgrade winner to
   MEDIUM. Two near-identical covers in the same candidate set means
   we can't be confident which is right — the user should pick
   explicitly.
5. **Format awareness (WP-5.6)** — see
   [Format / series-type awareness](#format--series-type-awareness)
   below. A known collected-vs-single (or annual-vs-either) mismatch
   subtracts `FORMAT_MISMATCH_PENALTY` (15) from the text `total` and
   caps the bucket at MEDIUM. It never moves MEDIUM or LOW.
6. **MatchOutcome classification** ([`api::metadata_search::build_match_outcome_view`](../../crates/server/src/api/metadata_search.rs))
   reduces the ranked list to one of five outcomes the dialog UX
   speaks: `single_good / multi_good / single_bad_cover /
   multi_bad_cover / no_match`.

### Format / series-type awareness

WP-5.6 (audit R13). Trades, annuals and manga volumes used to be
matched as if every file were a single periodical issue.

**Format classes.** Every vocabulary folds onto
[`title_norm::FormatClass`](../../crates/server/src/metadata/title_norm.rs)
(`Single` / `Annual` / `Collected`) through `classify_format`:

| Source | Signal |
|---|---|
| Metron | `series_type` on series details and (when present) issue list/detail series refs: `Ongoing Series`, `Limited Series`, `One-Shot` → Single; `Annual Series` → Annual; `Trade Paperback`, `Hard Cover`, `Omnibus`, `Graphic Novel` → Collected |
| ComicVine | CV volumes have **no type field**. `infer_format_from_title` reads the volume name (`TPB`, `TP`, `HC`, `Omnibus`, `Compendium`, `Graphic Novel`, `… Edition`, trailing `Annual`) and a deck that opens with `Collects…` |
| Local issue | `matcher::local_issue_format_hint`, first match wins: manga flag → neutral; ComicInfo `Format`; scanner `special_type` `TPB`/`Annual`; `series.series_type` |
| Local series | `series.series_type` |

Anything unrecognised is **unknown, and unknown never penalises**. Manga
(`issue.manga = Yes*`) is always neutral, because a manga "issue" is a
tankōbon volume that providers file as either ongoing issues or trades.
A `OneShot` special type is skipped too, because the scanner infers it
from a missing number.

**Penalty.** Both sides must be known and differ. Then
`Score.format = -FORMAT_MISMATCH_PENALTY` (a fixed 15 points, not
operator-tunable), `Score.format_mismatch = true`, and `Score::bucket`
demotes HIGH to MEDIUM. A perfect-text mismatch therefore lands at 65
(series) or 72.5 (issue). Both are below HIGH (80) but still MEDIUM
(≥ 60), so the candidate stays in the review queue. The cap also applies
when the cover decides HIGH, because a trade's cover is usually its
first issue's cover. It is the same shape as the gap-to-next-best guard.
The ladder constants are unchanged.

**Provider apply.** `GenericMetadata.format` is now populated with a
ComicInfo-vocabulary label (`TPB`, `Hardcover`, `Omnibus`,
`Graphic Novel`, `Annual`, `Limited Series`, `One-Shot`). Metron
*ongoing* and *cancelled* series map to `None`, so applying a periodical
never rewrites every file's `Format` to `Series`. ComicVine volumes also
fill `series_type` in Metron's vocabulary when a format is inferred.

**Issue numbers.** `title_norm::issue_number_key` is the comparison key.
It is numeric (`1` = `01` = `1.0`, `½` = `0.5` = `1/2`, `1½` = `1.5`),
keeps the suffix distinct (`14AU` = `14.AU` = `14 au` ≠ `14`), and
parses `Annual` (`Annual 1` = `annual #01` = `Ann. 1`) and volume
(`Vol. 03` = `v03` = `3`) markers. `matcher::canonical_issue_number`,
the provider-query form, gained the unambiguous subset: drop `#`,
`Annual N` spelling, drop volume prefixes, `014au` → `14AU`. Vulgar
fractions and dotted suffixes (`1.NOW`) pass through verbatim.

**Annuals.** ComicVine and Metron file `X-Men Annual #1` as issue `1`
of a separate `X-Men Annual` series.

- The issue scorer resolves annual-ness on each side from the number,
  the format, or an `Annual` word in the series name. When both sides
  are annual it compares the parent titles, and a candidate start year
  between the parent's start and this annual's cover year counts as a
  full year match.
- The orchestrator (`annual_query_rewrite`) searches
  `"<Series> Annual"` + `N`. It narrows only to a `series_provider_range`
  target, never to the parent series' default id, and year-gates on the
  annual's cover year.

**Article list.** The 23-word article list is unchanged. No fixture yet
shows a need for a language-aware list.

### Operator-tunable settings

All live under `/admin/metadata`'s Settings tab (driven by the
`metadata.*` keys in
[`settings/registry.rs`](../../crates/server/src/settings/registry.rs)).

| Key | Default | Effect |
|---|---|---|
| `metadata.auto_apply_threshold` | 80 | Text-score floor for HIGH bucket. ComicTagger reference value is 90 — tighten when text scoring proves consistent on your library; loosen when matches keep landing in MEDIUM. |
| `metadata.match_medium_threshold` | 60 | Text-score floor for MEDIUM bucket. Below this is LOW (hidden by default). |
| `metadata.alternate_cover_fetch_cap` | 3 | Max alternate-cover URLs fetched per candidate. Set to 0 to disable variant fetching entirely (primary only). Capped at 32 server-side. |

Per-library knobs (under `/admin/libraries/<slug>/settings`):

| Field | Default | Effect |
|---|---|---|
| `metadata_publisher_blacklist` | `[]` | Provider candidates from these publishers are dropped pre-scoring. Comparison is case-insensitive against the sanitized title form, so `"DC Comics"` / `"dc comics"` / `"DC"` all match the same entry. |
| `filename_ignore_leading_numbers` | false | Drop leading numeric token from filenames before inferring the series (closes `001 - Saga.cbz` curation case). |
| `filename_assume_issue_one` | false | When no issue number is detected, infer `#1` (closes one-shot / first-issue case). |

### Telemetry — `metadata_match_outcome`

Every completed search stamps one row capturing the outcome shape:

| Column | Source |
|---|---|
| `outcome_kind` | `MatchOutcomeKind::classify(&ranked)` — same 5-string vocabulary the dialog uses |
| `top_score` | Top candidate's `score.total` |
| `top_hamming` | Top candidate's cover Hamming when both phashes were available |
| `second_score` / `second_hamming` | Runner-up signals — drives the gap-to-next-best analysis |
| `candidate_count` | `ranked.len()` |
| `created_at` | timestamp; 90-day retention via nightly prune cron |

Operator dashboard: `/admin/metadata` "Match quality" card surfaces
rolling 7-day + 28-day distribution. Use it as the source-of-truth
metric when adjusting thresholds.

### Adding a regression-suite fixture

Tests in
[`crates/server/tests/matching_accuracy_golden.rs`](../../crates/server/tests/matching_accuracy_golden.rs)
anchor the matcher's accuracy invariants. Add a fixture when a real
production miss surfaces — that way the same case never regresses
silently.

**Recipe for a missed HIGH match** (operator reported a candidate
that should have been HIGH but landed MEDIUM/LOW):

1. Pull the run's `metadata_match_outcome` row from the dashboard
   "Runs" tab (or query directly):

   ```sql
   SELECT scope, outcome_kind, top_score, top_hamming, candidate_count
     FROM metadata_match_outcome
    WHERE run_id = '<run-id>';
   ```

2. Pull the candidate payload from `metadata_run_candidate.candidate`
   for `ordinal = 0` (top-ranked).

3. Reconstruct the `(SeriesQueryFacts, SeriesCandidate)` pair (or
   issue variant) and append it to the matching `known_correct_*`
   table in
   [`matching_accuracy_golden.rs`](../../crates/server/tests/matching_accuracy_golden.rs).

4. For cover-decided cases, use synthetic phashes when the real ones
   aren't trivially recoverable — the matcher only consumes the
   Hamming bit-distance, so `Some(0)` paired with `Some(0xF)`
   produces a 4-bit distance that bucketizes identically to two
   genuine pHashes. Pick values that hit the right Hamming bucket:

   | Bucket target | Bit-distance | Example phash pair |
   |---|---|---|
   | HIGH (cover-decides) | 0–8 | `Some(0)`, `Some(0xF)` (4 bits) |
   | MEDIUM (primary cover) | 9–16 | `Some(0)`, `Some(0x3FF)` (10 bits) |
   | MEDIUM (alternate cover) | 9–12 only | `Some(0)`, `Some(0x7FF)` (11 bits) |
   | LOW (cover veto) | 17+ | `Some(0)`, `Some(i64::MAX)` (~63 bits) |

5. Add the case name as the `name` field — it prints in assertion
   failure messages so a future regression names the broken case
   directly.

**Recipe for a false HIGH match** (operator reported the matcher
auto-applied the wrong thing): same recipe but append to
`known_incorrect_*`. The test asserts `bucket != HIGH` — any non-HIGH
classification passes.

### Reviewer heuristics

Cross-references the rules in [`CLAUDE.md`](../../CLAUDE.md)'s
"Matching engine" section. Reject PRs that:

- Add a weighted `cover_phash` bonus on top of text scoring (the M4
  inversion is intentional — cover decides the bucket, text is the
  fallback when no phash is present).
- Change the ComicTagger ladder constants (`STRONG_SCORE_THRESH`,
  `MIN_SCORE_THRESH`, `MIN_SCORE_DISTANCE`,
  `MIN_ALTERNATE_SCORE_THRESH`) without re-running the golden suite
  + adding fresh fixtures that exercise the boundary.
- Add a new bucket discriminant to `Confidence` without updating the
  operator-facing dialog copy + the `MatchOutcomeKind` vocabulary.
- Read the per-library publisher blacklist into pre-filter via
  anything other than `PreFilter::from_library` (the
  `as_array().filter_map(...).collect()` shape is the only safe
  path — operator-written non-array JSON would otherwise panic).
- Add a new operator-tunable threshold without a `metadata.*`
  registry entry + a default in `Config` + an `apply_setting`
  branch + a clamp on the upper bound.

### Plan reference

Full plan: [`~/.claude/plans/matching-accuracy-1.0.md`](../../../.claude/plans/matching-accuracy-1.0.md).
Slice 1 (M0 + M1 + M4) closes the "no HIGH matches" structural bug.
Slice 2 (M2 + M3 + M7) aligns the text pipeline with ComicTagger.
Slice 3 (M5 + M8 + M9) ships alternate-cover support, the dialog
state machine, and this regression suite. Slice 4 (M6 + M10 + M12)
covers smart cover-page selection, docs cross-cuts, and the opt-in
auto-apply path on `SingleGoodMatch`.
