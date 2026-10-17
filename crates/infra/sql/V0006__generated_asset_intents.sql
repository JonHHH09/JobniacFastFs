-- Preparation only. Existing owned assets remain UNCLASSIFIED: no adoption or backfill.
-- This table records generation continuation, not an expiry policy for completed outputs.
ALTER TABLE jobniac.generated_assets
    ADD COLUMN generation_intent_required boolean NOT NULL DEFAULT false;

-- Birth classification cannot be added by UPDATE, including a no-op UPDATE that
-- changes xmin. Deferred admission requires the intent in the birth transaction.
CREATE FUNCTION jobniac.guard_generation_birth_marker() RETURNS trigger
    LANGUAGE plpgsql SET search_path = pg_catalog AS $$
    BEGIN
        IF NEW.generation_intent_required IS DISTINCT FROM OLD.generation_intent_required THEN
            RAISE EXCEPTION 'Generation birth classification is immutable' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END $$;
CREATE TRIGGER generation_birth_marker_immutable
    BEFORE UPDATE ON jobniac.generated_assets
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generation_birth_marker();

CREATE TABLE jobniac.generation_intents (
    asset_id uuid PRIMARY KEY REFERENCES jobniac.generated_assets(asset_id) ON DELETE RESTRICT,
    operation_id uuid NOT NULL,
    purpose text NOT NULL CHECK (purpose IN (
        'STANDALONE_PDF', 'PROFILE_RESUME', 'GERMAN_RESUME', 'GERMAN_COVER_LETTER'
    )),
    profile_id uuid,
    job_id uuid,
    resume_id uuid,
    output_scope text,
    state text NOT NULL CHECK (state IN ('PENDING', 'COMPLETED', 'ABANDONED')),
    lease_token uuid,
    lease_started_at timestamptz,
    lease_expires_at timestamptz,
    created_at timestamptz NOT NULL,
    finished_at timestamptz,
    CONSTRAINT generation_intents_scope_shape CHECK (
        (purpose = 'STANDALONE_PDF' AND profile_id IS NULL AND job_id IS NULL
            AND resume_id IS NULL AND output_scope IS NULL)
        OR (purpose = 'PROFILE_RESUME' AND profile_id IS NOT NULL AND job_id IS NULL
            AND resume_id IS NULL AND output_scope IS NOT NULL
            AND output_scope IN ('master_resume', 'canadian_resume', 'canadian_resume_fr'))
        OR (purpose = 'GERMAN_RESUME'
            AND profile_id IS NOT NULL AND job_id IS NOT NULL AND resume_id IS NOT NULL
            AND output_scope IS NOT NULL AND output_scope IN ('en', 'de'))
        OR (purpose = 'GERMAN_COVER_LETTER'
            AND profile_id IS NOT NULL AND job_id IS NOT NULL AND resume_id IS NOT NULL
            AND output_scope IS NOT NULL AND output_scope = 'de')
    ),
    CONSTRAINT generation_intents_state_shape CHECK (
        (state = 'PENDING' AND lease_token IS NOT NULL AND lease_started_at IS NOT NULL
            AND lease_expires_at IS NOT NULL AND finished_at IS NULL
            AND lease_started_at >= created_at AND lease_expires_at > lease_started_at
            AND lease_expires_at <= lease_started_at + interval '5 minutes')
        OR (state IN ('COMPLETED', 'ABANDONED') AND lease_token IS NULL
            AND lease_started_at IS NULL AND lease_expires_at IS NULL
            AND finished_at IS NOT NULL AND finished_at >= created_at)
    )
);

-- A standalone operation has exactly one NULL-scoped output; bilingual operations
-- can have one output per language. The immutable asset supplies root/path/blob identity.
CREATE UNIQUE INDEX generation_intents_operation_output_unique
    ON jobniac.generation_intents(operation_id, purpose, output_scope) NULLS NOT DISTINCT;
CREATE INDEX generation_intents_pending_expiry_idx
    ON jobniac.generation_intents(lease_expires_at, asset_id) WHERE state = 'PENDING';

CREATE FUNCTION jobniac.check_generation_birth_intent() RETURNS trigger
    LANGUAGE plpgsql SET search_path = pg_catalog AS $$
    BEGIN
        IF NEW.generation_intent_required AND NOT EXISTS (
            SELECT 1 FROM jobniac.generation_intents WHERE asset_id = NEW.asset_id
        ) THEN
            RAISE EXCEPTION 'Classified reservation requires its birth intent'
                USING ERRCODE = '23514';
        END IF;
        RETURN NULL;
    END $$;
CREATE CONSTRAINT TRIGGER generation_birth_intent_required
    AFTER INSERT ON jobniac.generated_assets DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION jobniac.check_generation_birth_intent();

CREATE FUNCTION jobniac.guard_generation_intent() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    DECLARE
        doc uuid;
        path text;
        asset_state text;
        asset_created timestamptz;
        created_here boolean;
        classified_birth boolean;
        linked boolean;
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'Generation intent history must be retained' USING ERRCODE = '23514';
        END IF;
        SELECT document_id INTO doc FROM jobniac.generated_assets WHERE asset_id = NEW.asset_id;
        IF doc IS NULL THEN
            RAISE EXCEPTION 'Generation intent requires an owned document' USING ERRCODE = '23514';
        END IF;
        -- Never wait for an asset while an intent UPDATE already holds its row lock.
        -- Composed writers acquire owner -> sorted documents -> sorted assets -> intent.
        IF NOT pg_try_advisory_xact_lock(
            hashtextextended('jobniac-owned-document:' || doc::text, 0)
        ) OR NOT pg_try_advisory_xact_lock(jobniac.cleanup_asset_lock_key(NEW.asset_id)) THEN
            RAISE EXCEPTION 'Generation intent serialization is unavailable' USING ERRCODE = '23514';
        END IF;
        SELECT lifecycle, created_at, relative_path, generation_intent_required,
               xmin = pg_current_xact_id()::text::xid
            INTO asset_state, asset_created, path, classified_birth, created_here
            FROM jobniac.generated_assets WHERE asset_id = NEW.asset_id;
        IF TG_OP = 'INSERT' THEN
            IF NEW.state <> 'PENDING' OR asset_state <> 'RESERVED'
                OR NOT classified_birth OR NOT created_here
                OR NEW.created_at IS DISTINCT FROM asset_created
                OR NEW.lease_started_at IS DISTINCT FROM NEW.created_at THEN
                RAISE EXCEPTION 'Only a new reservation can acquire a generation intent'
                    USING ERRCODE = '23514';
            END IF;
        ELSE
            IF ROW(NEW.asset_id, NEW.operation_id, NEW.purpose, NEW.profile_id, NEW.job_id,
                   NEW.resume_id, NEW.output_scope, NEW.created_at)
                IS DISTINCT FROM
               ROW(OLD.asset_id, OLD.operation_id, OLD.purpose, OLD.profile_id, OLD.job_id,
                   OLD.resume_id, OLD.output_scope, OLD.created_at) THEN
                RAISE EXCEPTION 'Generation intent identity is immutable' USING ERRCODE = '23514';
            END IF;
            IF OLD.state <> 'PENDING' AND NEW IS DISTINCT FROM OLD THEN
                RAISE EXCEPTION 'Terminal generation intent is immutable' USING ERRCODE = '23514';
            END IF;
            IF OLD.state = 'PENDING' AND NEW IS DISTINCT FROM OLD THEN
                IF NEW.state = 'PENDING' AND (
                    NEW.lease_token IS NOT DISTINCT FROM OLD.lease_token
                    OR NEW.lease_started_at < OLD.lease_started_at
                    OR NEW.lease_started_at >= OLD.lease_expires_at
                    OR NEW.lease_expires_at < OLD.lease_expires_at
                ) THEN
                    RAISE EXCEPTION 'Generation renewal requires a live rotating fence'
                        USING ERRCODE = '23514';
                END IF;
                IF NEW.state IN ('COMPLETED', 'ABANDONED')
                    AND NEW.finished_at < OLD.lease_started_at THEN
                    RAISE EXCEPTION 'Generation finish predates its live fence'
                        USING ERRCODE = '23514';
                END IF;
                IF NEW.state = 'COMPLETED' AND NEW.finished_at >= OLD.lease_expires_at THEN
                    RAISE EXCEPTION 'Expired generation cannot complete' USING ERRCODE = '23514';
                END IF;
            END IF;
        END IF;
        IF NEW.state = 'PENDING' AND asset_state NOT IN ('RESERVED', 'ACTIVE') THEN
            RAISE EXCEPTION 'Pending generation requires an available owned asset'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.state = 'COMPLETED' AND (TG_OP = 'INSERT' OR OLD.state = 'PENDING') THEN
            IF asset_state <> 'ACTIVE' THEN
                RAISE EXCEPTION 'Generation completion requires publication proof'
                    USING ERRCODE = '23514';
            END IF;
            IF NEW.purpose = 'STANDALONE_PDF' THEN
                linked := true;
            ELSIF NEW.purpose = 'PROFILE_RESUME' THEN
                SELECT EXISTS (
                    SELECT 1 FROM profile.profile_resume_documents
                    WHERE profile_id = NEW.profile_id AND resume_type = NEW.output_scope
                        AND document_id = doc AND file_path = path
                ) INTO linked;
            ELSIF NEW.purpose = 'GERMAN_RESUME' THEN
                SELECT EXISTS (
                    SELECT 1 FROM resume.resume_variants variant
                    JOIN resume.resumes parent ON parent.id = variant.resume_id
                    WHERE parent.id = NEW.resume_id AND parent.profile_id = NEW.profile_id
                        AND parent.job_id = NEW.job_id AND parent.format = 'germany'
                        AND variant.language = NEW.output_scope
                        AND variant.document_id = doc AND variant.file_path = path
                ) INTO linked;
            ELSE
                SELECT EXISTS (
                    SELECT 1 FROM cover_letter.cover_letter_variants variant
                    JOIN cover_letter.cover_letters parent ON parent.id = variant.cover_letter_id
                    WHERE parent.resume_id = NEW.resume_id AND parent.profile_id = NEW.profile_id
                        AND parent.job_id = NEW.job_id AND variant.format = 'germany'
                        AND variant.language = NEW.output_scope
                        AND variant.document_id = doc AND variant.file_path = path
                ) INTO linked;
            END IF;
            IF NOT linked THEN
                RAISE EXCEPTION 'Generation completion requires its exact scoped link'
                    USING ERRCODE = '23514';
            END IF;
        END IF;
        RETURN NEW;
    END $$;

CREATE TRIGGER generation_intent_identity_guard
    BEFORE INSERT OR UPDATE OR DELETE ON jobniac.generation_intents
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_generation_intent();

-- Pending generation has its own explicit abandonment fence; it is not a
-- synthetic existing-link reference and ordinary cleanup cannot end its lease.
CREATE FUNCTION jobniac.guard_pending_generation_retirement() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    BEGIN
        IF NEW.lifecycle IN ('RETIRING', 'DELETED') AND EXISTS (
            SELECT 1 FROM jobniac.generation_intents
            WHERE asset_id = NEW.asset_id AND state = 'PENDING'
        ) THEN
            RAISE EXCEPTION 'Pending generation must be fenced before retirement'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END $$;

CREATE TRIGGER pending_generation_retirement_guard
    BEFORE UPDATE OF lifecycle ON jobniac.generated_assets
    FOR EACH ROW EXECUTE FUNCTION jobniac.guard_pending_generation_retirement();

CREATE FUNCTION jobniac.check_abandoned_generation_retirement() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog
    AS $$
    BEGIN
        IF EXISTS (
            SELECT 1 FROM jobniac.generation_intents intent
            JOIN jobniac.generated_assets asset ON asset.asset_id = intent.asset_id
            WHERE intent.asset_id = NEW.asset_id AND intent.state = 'ABANDONED'
                AND asset.lifecycle NOT IN ('RETIRING', 'DELETED')
        ) THEN
            RAISE EXCEPTION 'Abandoned generation requires atomic durable retirement'
                USING ERRCODE = '23514';
        END IF;
        RETURN NULL;
    END $$;

CREATE CONSTRAINT TRIGGER abandoned_generation_retirement_required
    AFTER INSERT OR UPDATE ON jobniac.generation_intents
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION jobniac.check_abandoned_generation_retirement();

-- Preserve EVERY prior reference class. Only completed standalone output adds
-- retention: completed profile/German intent rows are immutable history.
CREATE OR REPLACE FUNCTION jobniac.generated_document_referenced(doc uuid) RETURNS boolean
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
            OR EXISTS (
                SELECT 1 FROM jobniac.generation_intents intent
                JOIN jobniac.generated_assets asset ON asset.asset_id = intent.asset_id
                WHERE intent.state = 'COMPLETED' AND intent.purpose = 'STANDALONE_PDF'
                    AND asset.document_id = doc
            )
        )
    $$;
