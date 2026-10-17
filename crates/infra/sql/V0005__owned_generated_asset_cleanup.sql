-- Preparation only: no roots are registered and no legacy queue rows are changed.
-- Every filesystem operation must use the owning lifecycle adapter and this common lock.
CREATE TABLE jobniac.generated_asset_roots (
    id uuid PRIMARY KEY,
    created_at timestamptz NOT NULL
);

CREATE TABLE jobniac.generated_assets (
    asset_id uuid PRIMARY KEY,
    root_id uuid NOT NULL REFERENCES jobniac.generated_asset_roots(id) ON DELETE RESTRICT,
    relative_path text NOT NULL,
    sha256 char(64) NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    size_bytes bigint NOT NULL CHECK (size_bytes BETWEEN 1 AND 10485760),
    document_id uuid UNIQUE REFERENCES document.documents(id) ON DELETE RESTRICT,
    lifecycle text NOT NULL CHECK (lifecycle IN ('RESERVED', 'ACTIVE', 'RETIRING', 'DELETED')),
    created_at timestamptz NOT NULL,
    materialized_at timestamptz,
    retired_at timestamptz,
    deleted_at timestamptz,
    retirement_id uuid UNIQUE,
    UNIQUE (root_id, relative_path),
    UNIQUE (asset_id, retirement_id),
    CHECK (relative_path = asset_id::text || '.pdf'),
    CHECK ((lifecycle IN ('RETIRING', 'DELETED')) = (retirement_id IS NOT NULL)),
    CHECK ((lifecycle IN ('RETIRING', 'DELETED')) = (retired_at IS NOT NULL)),
    CHECK ((lifecycle = 'DELETED') = (deleted_at IS NOT NULL)),
    CHECK (lifecycle <> 'ACTIVE' OR materialized_at IS NOT NULL)
);

CREATE TABLE jobniac.generated_asset_cleanups (
    task_id uuid PRIMARY KEY,
    asset_id uuid NOT NULL UNIQUE,
    retirement_id uuid NOT NULL,
    status text NOT NULL CHECK (status IN ('PENDING', 'PROCESSING', 'COMPLETED', 'BLOCKED')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at timestamptz NOT NULL,
    lease_token uuid,
    lease_expires_at timestamptz,
    failure_category text CHECK (failure_category IN (
        'FILE_DELETE_FAILED', 'IDENTITY_MISMATCH', 'OWNERSHIP_UNPROVEN', 'UNSUPPORTED_PLATFORM'
    )),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    completed_at timestamptz,
    FOREIGN KEY (asset_id, retirement_id)
        REFERENCES jobniac.generated_assets(asset_id, retirement_id) ON DELETE RESTRICT,
    CHECK ((status = 'PROCESSING') = (lease_token IS NOT NULL)),
    CHECK ((status = 'PROCESSING') = (lease_expires_at IS NOT NULL)),
    CHECK ((status = 'COMPLETED') = (completed_at IS NOT NULL))
);

CREATE INDEX generated_asset_cleanups_due_idx
    ON jobniac.generated_asset_cleanups(next_attempt_at, task_id)
    WHERE status IN ('PENDING', 'PROCESSING');
CREATE INDEX generated_asset_cleanups_completed_idx
    ON jobniac.generated_asset_cleanups(completed_at, task_id)
    WHERE status = 'COMPLETED';

CREATE FUNCTION jobniac.cleanup_asset_lock_key(asset uuid) RETURNS bigint
    LANGUAGE sql IMMUTABLE STRICT
    SET search_path = pg_catalog
    AS $$ SELECT hashtextextended('jobniac-owned-asset:' || asset::text, 0) $$;

CREATE FUNCTION jobniac.generated_document_referenced(doc uuid) RETURNS boolean
    LANGUAGE sql STABLE
    SET search_path = pg_catalog
    AS $$
        SELECT doc IS NOT NULL AND (
            EXISTS (SELECT 1 FROM profile.profile_resume_documents WHERE document_id = doc)
            OR EXISTS (SELECT 1 FROM resume.resume_variants WHERE document_id = doc)
            OR EXISTS (SELECT 1 FROM cover_letter.cover_letter_variants WHERE document_id = doc)
            OR EXISTS (SELECT 1 FROM profile.profile_personal_details WHERE photo_document_id = doc)
            OR EXISTS (
                SELECT 1 FROM profile.profile_pdf_sources source
                JOIN document.pdf_extractions extraction
                    ON extraction.id = source.pdf_extraction_id
                WHERE extraction.file_id = doc
            )
        )
    $$;

CREATE FUNCTION jobniac.generated_asset_referenced(doc uuid, path text) RETURNS boolean
    LANGUAGE sql STABLE
    SET search_path = pg_catalog
    AS $$ SELECT jobniac.generated_document_referenced(doc)
        OR EXISTS (SELECT 1 FROM profile.profile_resume_documents WHERE file_path = path)
        OR EXISTS (SELECT 1 FROM resume.resume_variants WHERE file_path = path)
        OR EXISTS (SELECT 1 FROM cover_letter.cover_letter_variants WHERE file_path = path)
    $$;

CREATE FUNCTION jobniac.guard_generated_asset() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'Owned asset tombstones must be retained' USING ERRCODE = '23514';
        END IF;
        IF TG_OP = 'INSERT' AND NEW.document_id IS NOT NULL THEN
            -- The document lock protects reservation against an attachment that
            -- started before the document had an owned-asset mapping.
            PERFORM pg_advisory_xact_lock(
                hashtextextended('jobniac-owned-document:' || NEW.document_id::text, 0)
            );
        END IF;
        PERFORM pg_advisory_xact_lock(jobniac.cleanup_asset_lock_key(NEW.asset_id));
        IF TG_OP = 'INSERT' THEN
            IF NEW.lifecycle <> 'RESERVED' OR NEW.retirement_id IS NOT NULL THEN
                RAISE EXCEPTION 'Owned assets begin with durable reservation' USING ERRCODE = '23514';
            END IF;
            IF jobniac.generated_asset_referenced(NEW.document_id, NEW.relative_path) THEN
                RAISE EXCEPTION 'Existing references cannot be adopted as owned' USING ERRCODE = '23514';
            END IF;
            IF NEW.document_id IS NOT NULL AND NOT EXISTS (
                SELECT 1 FROM document.documents file
                JOIN document.blobs blob ON blob.id = file.blob_id
                WHERE file.id = NEW.document_id AND blob.sha256 = NEW.sha256
                    AND blob.byte_size = NEW.size_bytes
                FOR SHARE OF blob
            ) THEN
                RAISE EXCEPTION 'Owned document identity is inconsistent' USING ERRCODE = '23514';
            END IF;
        ELSE
            IF ROW(NEW.asset_id, NEW.root_id, NEW.relative_path, NEW.sha256,
                   NEW.size_bytes, NEW.document_id, NEW.created_at)
                IS DISTINCT FROM
               ROW(OLD.asset_id, OLD.root_id, OLD.relative_path, OLD.sha256,
                   OLD.size_bytes, OLD.document_id, OLD.created_at) THEN
                RAISE EXCEPTION 'Owned asset identity is immutable' USING ERRCODE = '23514';
            END IF;
            IF OLD.retirement_id IS NOT NULL AND (
                NEW.retirement_id IS DISTINCT FROM OLD.retirement_id
                OR NEW.retired_at IS DISTINCT FROM OLD.retired_at
            ) THEN
                RAISE EXCEPTION 'Retirement identity is permanent' USING ERRCODE = '23514';
            END IF;
            IF NOT (
                NEW.lifecycle = OLD.lifecycle
                OR (OLD.lifecycle = 'RESERVED' AND NEW.lifecycle IN ('ACTIVE', 'RETIRING'))
                OR (OLD.lifecycle = 'ACTIVE' AND NEW.lifecycle = 'RETIRING')
                OR (OLD.lifecycle = 'RETIRING' AND NEW.lifecycle = 'DELETED')
            ) THEN
                RAISE EXCEPTION 'Owned asset lifecycle cannot regress' USING ERRCODE = '23514';
            END IF;
            IF NEW.lifecycle IN ('RETIRING', 'DELETED')
                AND jobniac.generated_asset_referenced(NEW.document_id, NEW.relative_path) THEN
                RAISE EXCEPTION 'Referenced generated assets cannot retire' USING ERRCODE = '23514';
            END IF;
            IF NEW.lifecycle = 'DELETED' AND OLD.lifecycle <> 'DELETED' AND NOT EXISTS (
                SELECT 1 FROM jobniac.generated_asset_cleanups task
                WHERE task.asset_id = NEW.asset_id AND task.retirement_id = NEW.retirement_id
                    AND task.status = 'COMPLETED'
            ) THEN
                RAISE EXCEPTION 'Owned deletion requires completed durable intent'
                    USING ERRCODE = '23514';
            END IF;
        END IF;
        RETURN NEW;
    END $$;

CREATE TRIGGER generated_asset_lifecycle_guard
    BEFORE INSERT OR UPDATE OR DELETE ON jobniac.generated_assets
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generated_asset();

CREATE FUNCTION jobniac.guard_generated_cleanup() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    DECLARE
        target uuid;
    BEGIN
        target := CASE WHEN TG_OP = 'DELETE' THEN OLD.asset_id ELSE NEW.asset_id END;
        PERFORM pg_advisory_xact_lock(jobniac.cleanup_asset_lock_key(target));
        IF TG_OP = 'DELETE' THEN
            IF OLD.status <> 'COMPLETED' OR NOT EXISTS (
                SELECT 1 FROM jobniac.generated_assets WHERE asset_id = target
                    AND lifecycle = 'DELETED' AND retirement_id = OLD.retirement_id
            ) THEN
                RAISE EXCEPTION 'Unresolved cleanup intent must be retained' USING ERRCODE = '23514';
            END IF;
            RETURN OLD;
        END IF;
        IF TG_OP = 'UPDATE' AND
            ROW(NEW.task_id, NEW.asset_id, NEW.retirement_id, NEW.created_at)
            IS DISTINCT FROM ROW(OLD.task_id, OLD.asset_id, OLD.retirement_id, OLD.created_at) THEN
            RAISE EXCEPTION 'Cleanup intent identity is immutable' USING ERRCODE = '23514';
        END IF;
        IF TG_OP = 'UPDATE' AND OLD.status = 'COMPLETED' AND NEW IS DISTINCT FROM OLD THEN
            RAISE EXCEPTION 'Completed cleanup intent is immutable' USING ERRCODE = '23514';
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM jobniac.generated_assets WHERE asset_id = target
                AND retirement_id = NEW.retirement_id
                AND lifecycle IN ('RETIRING', 'DELETED')
        ) THEN
            RAISE EXCEPTION 'Cleanup requires permanent retired identity' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END $$;

CREATE TRIGGER generated_cleanup_identity_guard
    BEFORE INSERT OR UPDATE OR DELETE ON jobniac.generated_asset_cleanups
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generated_cleanup();

CREATE FUNCTION jobniac.assert_generated_document_attachable(doc uuid) RETURNS void
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    DECLARE
        asset uuid;
        state text;
    BEGIN
        IF doc IS NULL THEN RETURN; END IF;
        PERFORM pg_advisory_xact_lock(
            hashtextextended('jobniac-owned-document:' || doc::text, 0)
        );
        SELECT asset_id INTO asset FROM jobniac.generated_assets WHERE document_id = doc;
        IF asset IS NULL THEN RETURN; END IF;
        PERFORM pg_advisory_xact_lock(jobniac.cleanup_asset_lock_key(asset));
        -- This separate statement sees retirement committed while lock acquisition waited.
        SELECT lifecycle INTO state FROM jobniac.generated_assets WHERE asset_id = asset;
        IF state IS DISTINCT FROM 'ACTIVE' THEN
            RAISE EXCEPTION 'Generated document is unavailable for attachment' USING ERRCODE = '23514';
        END IF;
    END $$;

CREATE FUNCTION jobniac.guard_generated_document_reference() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    DECLARE
        doc uuid;
        mapped_asset uuid;
        mapped_doc uuid;
        document_asset uuid;
        path_asset uuid;
        target uuid;
        path text;
    BEGIN
        doc := (to_jsonb(NEW) ->> TG_ARGV[0])::uuid;
        -- Match reservation's document-then-asset lock order. A future UUID path
        -- must participate even before an ownership mapping exists.
        IF doc IS NOT NULL THEN
            PERFORM pg_advisory_xact_lock(
                hashtextextended('jobniac-owned-document:' || doc::text, 0)
            );
        END IF;
        path := to_jsonb(NEW) ->> 'file_path';
        IF path ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}[.]pdf$' THEN
            path_asset := left(path, length(path) - 4)::uuid;
        END IF;
        SELECT asset_id INTO document_asset FROM jobniac.generated_assets WHERE document_id = doc;
        -- Two different identities are locked in UUID order before path validation
        -- rejects rebinding; reciprocal invalid path writes cannot deadlock here.
        FOR target IN SELECT DISTINCT value FROM unnest(ARRAY[path_asset, document_asset]) value
            WHERE value IS NOT NULL ORDER BY value LOOP
            PERFORM pg_advisory_xact_lock(jobniac.cleanup_asset_lock_key(target));
        END LOOP;
        PERFORM jobniac.assert_generated_document_attachable(doc);
        IF (TG_TABLE_SCHEMA = 'profile' AND TG_TABLE_NAME = 'profile_resume_documents')
            OR (TG_TABLE_SCHEMA = 'resume' AND TG_TABLE_NAME = 'resume_variants')
            OR (TG_TABLE_SCHEMA = 'cover_letter' AND TG_TABLE_NAME = 'cover_letter_variants') THEN
            SELECT asset_id INTO mapped_asset FROM jobniac.generated_assets
                WHERE relative_path = NEW.file_path;
            IF mapped_asset IS NOT NULL THEN
                PERFORM pg_advisory_xact_lock(jobniac.cleanup_asset_lock_key(mapped_asset));
                SELECT document_id INTO mapped_doc FROM jobniac.generated_assets
                    WHERE asset_id = mapped_asset;
                IF mapped_doc IS NULL OR mapped_doc IS DISTINCT FROM doc THEN
                    RAISE EXCEPTION 'Generated paths cannot be rebound' USING ERRCODE = '23514';
                END IF;
            END IF;
            IF EXISTS (SELECT 1 FROM jobniac.generated_assets
                       WHERE document_id = doc AND relative_path <> NEW.file_path) THEN
                RAISE EXCEPTION 'Generated document path is immutable' USING ERRCODE = '23514';
            END IF;
        END IF;
        RETURN NEW;
    END $$;

CREATE TRIGGER owned_profile_resume_reference_guard
    BEFORE INSERT OR UPDATE ON profile.profile_resume_documents
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generated_document_reference('document_id');
CREATE TRIGGER owned_resume_variant_reference_guard
    BEFORE INSERT OR UPDATE ON resume.resume_variants
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generated_document_reference('document_id');
CREATE TRIGGER owned_cover_letter_variant_reference_guard
    BEFORE INSERT OR UPDATE ON cover_letter.cover_letter_variants
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generated_document_reference('document_id');
CREATE TRIGGER owned_profile_photo_reference_guard
    BEFORE INSERT OR UPDATE OF photo_document_id ON profile.profile_personal_details
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generated_document_reference('photo_document_id');
CREATE TRIGGER owned_pdf_extraction_reference_guard
    BEFORE INSERT OR UPDATE OF file_id ON document.pdf_extractions
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generated_document_reference('file_id');

CREATE FUNCTION jobniac.guard_generated_pdf_source() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    DECLARE doc uuid;
    BEGIN
        -- Keep the resolved document stable through validation and source commit.
        -- Extraction file_id updates take the same row's conflicting write lock.
        SELECT file_id INTO doc FROM document.pdf_extractions
            WHERE id = NEW.pdf_extraction_id FOR SHARE;
        PERFORM jobniac.assert_generated_document_attachable(doc);
        RETURN NEW;
    END $$;

CREATE TRIGGER owned_pdf_source_reference_guard
    BEFORE INSERT OR UPDATE OF pdf_extraction_id ON profile.profile_pdf_sources
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generated_pdf_source();

CREATE FUNCTION jobniac.guard_owned_document_content() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    BEGIN
        PERFORM pg_advisory_xact_lock(
            hashtextextended('jobniac-owned-document:' || OLD.id::text, 0)
        );
        IF NEW.blob_id IS DISTINCT FROM OLD.blob_id
            AND EXISTS (SELECT 1 FROM jobniac.generated_assets WHERE document_id = OLD.id) THEN
            RAISE EXCEPTION 'Owned document content cannot be rebound' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END $$;

CREATE TRIGGER owned_document_content_guard
    BEFORE UPDATE OF blob_id ON document.documents
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_owned_document_content();

CREATE FUNCTION jobniac.guard_owned_blob_content() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    DECLARE doc uuid;
    BEGIN
        IF ROW(NEW.id, NEW.sha256, NEW.byte_size, NEW.content)
            IS NOT DISTINCT FROM ROW(OLD.id, OLD.sha256, OLD.byte_size, OLD.content) THEN
            RETURN NEW;
        END IF;
        FOR doc IN SELECT id FROM document.documents WHERE blob_id = OLD.id ORDER BY id LOOP
            PERFORM pg_advisory_xact_lock(
                hashtextextended('jobniac-owned-document:' || doc::text, 0)
            );
        END LOOP;
        IF EXISTS (
            SELECT 1 FROM jobniac.generated_assets asset
            JOIN document.documents file ON file.id = asset.document_id
            WHERE file.blob_id = OLD.id
        ) THEN
            RAISE EXCEPTION 'Owned document blob identity is immutable' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END $$;

CREATE TRIGGER owned_blob_content_guard
    BEFORE UPDATE ON document.blobs
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_owned_blob_content();
