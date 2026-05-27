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
5. **MatchOutcome classification** ([`api::metadata_search::build_match_outcome_view`](../../crates/server/src/api/metadata_search.rs))
   reduces the ranked list to one of five outcomes the dialog UX
   speaks: `single_good / multi_good / single_bad_cover /
   multi_bad_cover / no_match`.

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
