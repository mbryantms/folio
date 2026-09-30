-- Per-user activity for `just perf-explain` (scripts/perf/perf-explain.sh).
--
-- The scan seeds the catalogue (series / issues / junctions / credits);
-- this file layers on what a long-lived reader account looks like so the
-- per-user list queries (continue reading, on deck, CBL entries, saved
-- views, collections) have realistic row counts to plan against.
-- Deterministic (derived from series ordinal) and idempotent: every row
-- it owns is deleted and re-inserted on each run.
--
-- psql var: :user_id — the perf admin user.

BEGIN;

DELETE FROM progress_records
WHERE user_id = :'user_id'
   OR user_id IN (SELECT id FROM users WHERE email LIKE 'perf-other-%@example.com');
DELETE FROM saved_views WHERE user_id = :'user_id' AND name LIKE 'perf: %';
DELETE FROM cbl_lists WHERE parsed_name LIKE 'perf: %';

-- Series in a stable order, numbered.
CREATE TEMP TABLE perf_series ON COMMIT DROP AS
SELECT s.id, row_number() OVER (ORDER BY s.normalized_name, s.id) AS ord
FROM series s;

CREATE TEMP TABLE perf_issues ON COMMIT DROP AS
SELECT i.id, i.series_id, ps.ord,
       row_number() OVER (PARTITION BY i.series_id ORDER BY i.sort_number, i.id) AS n
FROM issues i JOIN perf_series ps ON ps.id = i.series_id
WHERE i.state = 'active' AND i.removed_at IS NULL;

-- Reading history: every 5th series (≈20 % of the library) has a finished
-- prefix of 1..15 issues; half of those also have an in-progress issue
-- right after the prefix (→ Continue Reading), the other half don't
-- (→ On Deck "next unread in series").
INSERT INTO progress_records (user_id, issue_id, last_page, percent, finished,
                              updated_at, finished_at, device, is_backfill, run)
SELECT :'user_id'::uuid, pi.id,
       CASE WHEN pi.n <= 1 + (pi.ord % 15) THEN 1 ELSE 0 END + 1,
       CASE WHEN pi.n <= 1 + (pi.ord % 15) THEN 1.0 ELSE 0.5 END,
       pi.n <= 1 + (pi.ord % 15),
       now() - make_interval(mins => (pi.ord * 37 + pi.n)::int),
       CASE WHEN pi.n <= 1 + (pi.ord % 15)
            THEN now() - make_interval(mins => (pi.ord * 37 + pi.n)::int) END,
       'perf', false, 0
FROM perf_issues pi
WHERE pi.ord % 5 = 0
  AND (pi.n <= 1 + (pi.ord % 15)
       OR (pi.n = 2 + (pi.ord % 15) AND pi.ord % 10 = 0));

-- The four other readers get the same shape over the *other* 80 % of the
-- library (ord % 5 = k), so the caller owns ~20 % of progress_records.
INSERT INTO progress_records (user_id, issue_id, last_page, percent, finished,
                              updated_at, finished_at, device, is_backfill, run)
SELECT u.id, pi.id, 2, 1.0, true,
       now() - make_interval(mins => (pi.ord * 41 + pi.n)::int),
       now() - make_interval(mins => (pi.ord * 41 + pi.n)::int),
       'perf', false, 0
FROM perf_issues pi
JOIN (SELECT id, row_number() OVER (ORDER BY email) AS k
      FROM users WHERE email LIKE 'perf-other-%@example.com') u
  ON pi.ord % 5 = u.k
WHERE pi.n <= 1 + (pi.ord % 15);

-- A 600-entry CBL ("event reading order"): issues 1-3 of 200 series spread
-- across the library, plus 40 unmatched entries.
INSERT INTO cbl_lists (id, owner_user_id, source_kind, raw_sha256, raw_xml,
                       parsed_name, parsed_matchers_present, num_issues_declared)
VALUES (gen_random_uuid(), :'user_id', 'upload', '\x00'::bytea, '<ReadingList/>',
        'perf: event reading order', false, 640);

INSERT INTO cbl_entries (id, cbl_list_id, position, series_name, issue_number,
                         matched_issue_id, match_status, match_method, matched_at)
SELECT gen_random_uuid(), l.id, (row_number() OVER (ORDER BY pi.n, pi.ord) - 1)::int,
       'Series', pi.n::text, pi.id, 'matched', 'name', now()
FROM perf_issues pi
CROSS JOIN (SELECT id FROM cbl_lists WHERE parsed_name = 'perf: event reading order') l
WHERE pi.ord % 12 = 3 AND pi.ord <= 12 * 200 AND pi.n <= 3;

INSERT INTO cbl_entries (id, cbl_list_id, position, series_name, issue_number, match_status)
SELECT gen_random_uuid(), l.id,
       (SELECT count(*) FROM cbl_entries e WHERE e.cbl_list_id = l.id)::int + g - 1,
       'Missing Series', g::text, 'missing'
FROM generate_series(1, 40) g
CROSS JOIN (SELECT id FROM cbl_lists WHERE parsed_name = 'perf: event reading order') l;

-- The CBL as a saved view (what the sidebar pins / the reading-list page read).
INSERT INTO saved_views (id, user_id, kind, name, cbl_list_id)
SELECT gen_random_uuid(), :'user_id', 'cbl', 'perf: event reading order', id
FROM cbl_lists WHERE parsed_name = 'perf: event reading order';

-- A filter view: publisher + year window, name sort.
INSERT INTO saved_views (id, user_id, kind, name, match_mode, conditions,
                         sort_field, sort_order, result_limit)
VALUES (gen_random_uuid(), :'user_id', 'filter_series', 'perf: publisher+year', 'all',
        '[{"group_id":0,"field":"publisher","op":"equals","value":"Load Comics"},
          {"group_id":0,"field":"year","op":"between","value":[2005,2015]}]'::jsonb,
        'name', 'asc', 50);

-- A 300-entry manual collection: 150 series + 150 issues.
INSERT INTO saved_views (id, user_id, kind, name)
VALUES (gen_random_uuid(), :'user_id', 'collection', 'perf: collection');

INSERT INTO collection_entries (id, saved_view_id, position, entry_kind, series_id, issue_id)
SELECT gen_random_uuid(), v.id, (row_number() OVER (ORDER BY x.k) - 1)::int,
       x.kind, x.series_id, x.issue_id
FROM (
    SELECT ps.ord * 2 AS k, 'series' AS kind, ps.id AS series_id, NULL::text AS issue_id
    FROM perf_series ps WHERE ps.ord % 16 = 1 AND ps.ord <= 16 * 150
    UNION ALL
    SELECT pi.ord * 2 + 1, 'issue', NULL, pi.id
    FROM perf_issues pi WHERE pi.ord % 16 = 9 AND pi.ord <= 16 * 150 AND pi.n = 1
) x
CROSS JOIN (SELECT id FROM saved_views
            WHERE user_id = :'user_id' AND name = 'perf: collection') v;

COMMIT;
