-- Duplicate issues for `just perf-explain` (scripts/perf/perf-explain.sh).
--
-- The stress fixture has no duplicate files, so without this the
-- Duplicates page returns an empty first page and its deep-page entry is
-- skipped. This copies issue #1 of every 10th series as a second row
-- (250 `number` groups); every 5th copy keeps the original `content_hash`
-- so 50 of those are also `hash` groups, which the all-kinds listing
-- suppresses as subsets. Rows only — no file is written: the Duplicates
-- page reads the catalogue, and `file_path` is never opened by it.
--
-- Deterministic and idempotent: the copies are tagged by slug and deleted
-- before they are re-inserted.

BEGIN;

DELETE FROM issues WHERE slug LIKE '%-perf-dup';

CREATE TEMP TABLE perf_dup_series ON COMMIT DROP AS
SELECT s.id, row_number() OVER (ORDER BY s.normalized_name, s.id) AS ord
FROM series s;

-- Same columns as `issues`, so the copy below needs no column list.
CREATE TEMP TABLE perf_dups ON COMMIT DROP AS
SELECT i.*
FROM issues i
JOIN perf_dup_series ps ON ps.id = i.series_id
WHERE ps.ord % 10 = 0
  AND i.sort_number = 1
  AND i.special_type IS NULL
  AND i.state = 'active'
  AND i.removed_at IS NULL;

UPDATE perf_dups d
SET id           = encode(sha256(('perf-dup:' || d.id)::bytea), 'hex'),
    slug         = d.slug || '-perf-dup',
    file_path    = regexp_replace(d.file_path, '\.cbz$', ' (perf dup).cbz'),
    content_hash = CASE
        WHEN ps.ord % 50 = 0 THEN d.content_hash                       -- hash + number group
        ELSE encode(sha256(('perf-dup-hash:' || d.id)::bytea), 'hex') -- number group only
    END
FROM perf_dup_series ps
WHERE ps.id = d.series_id;

-- `issues.search_doc` is a STORED generated column, so the copy lists the
-- plain columns explicitly (looked up, not hand-maintained).
DO $$
DECLARE
    cols text;
BEGIN
    SELECT string_agg(quote_ident(column_name), ', ' ORDER BY ordinal_position)
      INTO cols
      FROM information_schema.columns
     WHERE table_schema = 'public' AND table_name = 'issues' AND is_generated = 'NEVER';
    EXECUTE format('INSERT INTO issues (%s) SELECT %s FROM perf_dups', cols, cols);
END $$;

COMMIT;
