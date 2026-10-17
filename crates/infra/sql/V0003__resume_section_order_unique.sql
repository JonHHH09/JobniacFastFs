-- Preserve existing rows: duplicate positions require an explicit operator resolution.
-- The migration runner applies this script and its checksum history in one transaction.
LOCK TABLE resume.resume_sections IN ACCESS EXCLUSIVE MODE;

DO $migration$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM resume.resume_sections
        GROUP BY variant_id, display_order
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23505',
            MESSAGE = 'resume section positions must be unique within each variant',
            CONSTRAINT = 'resume_sections_variant_order_unique';
    END IF;
END;
$migration$;

ALTER TABLE resume.resume_sections
    ADD CONSTRAINT resume_sections_variant_order_unique UNIQUE (variant_id, display_order);
