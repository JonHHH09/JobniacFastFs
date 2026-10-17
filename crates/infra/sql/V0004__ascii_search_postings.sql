-- Explicit upgrade for Python IGNORECASE's surviving non-ASCII letter, dotless i.
-- Other exceptional letters normalize to ASCII before tokenization. Source Spring
-- postings already use the ASCII alphabet and are deliberately left untouched.
-- This script and its history row run in one migration transaction.
LOCK TABLE profile.search_terms, job_schema.search_terms IN SHARE ROW EXCLUSIVE MODE;

CREATE TEMP TABLE jobniac_v4_profile_terms ON COMMIT DROP AS
SELECT posting.profile_id, posting.field_key, part.term, posting.weight
FROM profile.search_terms AS posting
CROSS JOIN LATERAL regexp_split_to_table(posting.term, 'ı') AS part(term)
WHERE position('ı' in posting.term) > 0 AND part.term <> '';

CREATE TEMP TABLE jobniac_v4_job_terms ON COMMIT DROP AS
SELECT posting.job_id, posting.field_key, part.term, posting.weight
FROM job_schema.search_terms AS posting
CROSS JOIN LATERAL regexp_split_to_table(posting.term, 'ı') AS part(term)
WHERE position('ı' in posting.term) > 0 AND part.term <> '';

DO $migration$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_temp.jobniac_v4_profile_terms
        GROUP BY profile_id, field_key, term HAVING min(weight) <> max(weight)
    ) OR EXISTS (
        SELECT 1 FROM pg_temp.jobniac_v4_job_terms
        GROUP BY job_id, field_key, term HAVING min(weight) <> max(weight)
    ) OR EXISTS (
        SELECT 1 FROM pg_temp.jobniac_v4_profile_terms AS converted
        JOIN profile.search_terms AS existing
          USING (profile_id, field_key, term)
        WHERE existing.weight <> converted.weight
    ) OR EXISTS (
        SELECT 1 FROM pg_temp.jobniac_v4_job_terms AS converted
        JOIN job_schema.search_terms AS existing
          USING (job_id, field_key, term)
        WHERE existing.weight <> converted.weight
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23514',
            MESSAGE = 'search posting collision weights must agree';
    END IF;
END;
$migration$;

DELETE FROM profile.search_terms WHERE position('ı' in term) > 0;
INSERT INTO profile.search_terms (profile_id, field_key, term, weight)
SELECT profile_id, field_key, term, max(weight)
FROM pg_temp.jobniac_v4_profile_terms GROUP BY profile_id, field_key, term
ON CONFLICT (profile_id, field_key, term) DO NOTHING;

DELETE FROM job_schema.search_terms WHERE position('ı' in term) > 0;
INSERT INTO job_schema.search_terms (job_id, field_key, term, weight)
SELECT job_id, field_key, term, max(weight)
FROM pg_temp.jobniac_v4_job_terms GROUP BY job_id, field_key, term
ON CONFLICT (job_id, field_key, term) DO NOTHING;
