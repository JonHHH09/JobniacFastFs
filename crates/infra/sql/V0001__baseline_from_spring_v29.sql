-- Jobniac baseline schema (V0001).
--
-- Derived mechanically from a pg_dump --schema-only of an empty PostgreSQL 18 database that
-- the job-engine-spring reference jar migrated through its own Flyway V1..V29 sequence
-- (source evidence: jar sha256 84a3ec63c109f1bf5465ea1a498217b8064cece54a91c5b6d6dc529e8b6c5b96, Flyway history rank 29 = V29).
-- The Spring Flyway files are source history only and are never replayed; Flyway's
-- profile.flyway_schema_history bookkeeping and pg_dump session settings are omitted.
-- Every object is schema-qualified; the runner applies this file in one transaction.

CREATE SCHEMA cover_letter;

CREATE SCHEMA document;

CREATE SCHEMA job_schema;

CREATE SCHEMA match;

CREATE SCHEMA profile;

CREATE SCHEMA resume;

CREATE FUNCTION job_schema.assert_job_source_provenance(target_job_id uuid) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE
    job_source_method text;
    link_count integer;
    text_count integer;
BEGIN
    IF target_job_id IS NULL THEN
        RETURN;
    END IF;
    SELECT source_method
    INTO job_source_method
    FROM job_schema.jobs
    WHERE id = target_job_id;
    IF job_source_method IS NULL THEN
        RETURN;
    END IF;
    SELECT count(*) INTO link_count
    FROM job_schema.job_link_ingestions
    WHERE job_id = target_job_id;
    SELECT count(*) INTO text_count
    FROM job_schema.job_text_ingestions
    WHERE job_id = target_job_id;
    IF job_source_method = 'link' AND (link_count <> 1 OR text_count <> 0) THEN
        RAISE EXCEPTION 'job % must have exactly one link provenance row and no text provenance rows', target_job_id;
    END IF;
    IF job_source_method = 'text' AND (text_count <> 1 OR link_count <> 0) THEN
        RAISE EXCEPTION 'job % must have exactly one text provenance row and no link provenance rows', target_job_id;
    END IF;
    RETURN;
END
$$;

CREATE FUNCTION job_schema.enforce_job_source_provenance() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    new_job_id uuid;
    old_job_id uuid;
BEGIN
    new_job_id := COALESCE(
        (to_jsonb(NEW) ->> 'job_id')::uuid,
        (to_jsonb(NEW) ->> 'id')::uuid
    );
    old_job_id := COALESCE(
        (to_jsonb(OLD) ->> 'job_id')::uuid,
        (to_jsonb(OLD) ->> 'id')::uuid
    );
    PERFORM job_schema.assert_job_source_provenance(new_job_id);
    IF old_job_id IS DISTINCT FROM new_job_id THEN
        PERFORM job_schema.assert_job_source_provenance(old_job_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE FUNCTION match.reject_immutable_update() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN RAISE EXCEPTION 'match history is immutable'; END $$;

CREATE TABLE cover_letter.cover_letter_paragraphs (
    id uuid NOT NULL,
    variant_id uuid NOT NULL,
    display_order integer NOT NULL,
    text text NOT NULL,
    CONSTRAINT cover_letter_paragraphs_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT cover_letter_paragraphs_text_not_blank CHECK ((btrim(text) <> ''::text))
);

CREATE TABLE cover_letter.cover_letter_variants (
    id uuid NOT NULL,
    cover_letter_id uuid NOT NULL,
    format text NOT NULL,
    language text NOT NULL,
    document_id uuid NOT NULL,
    file_path text NOT NULL,
    subject text NOT NULL,
    salutation text NOT NULL,
    closing text NOT NULL,
    signature text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT cover_letter_variants_closing_not_blank CHECK ((btrim(closing) <> ''::text)),
    CONSTRAINT cover_letter_variants_file_path_not_blank CHECK ((btrim(file_path) <> ''::text)),
    CONSTRAINT cover_letter_variants_format_known CHECK ((format = 'germany'::text)),
    CONSTRAINT cover_letter_variants_language_known CHECK ((language = 'de'::text)),
    CONSTRAINT cover_letter_variants_salutation_not_blank CHECK ((btrim(salutation) <> ''::text)),
    CONSTRAINT cover_letter_variants_signature_not_blank CHECK ((btrim(signature) <> ''::text)),
    CONSTRAINT cover_letter_variants_subject_not_blank CHECK ((btrim(subject) <> ''::text)),
    CONSTRAINT cover_letter_variants_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

CREATE TABLE cover_letter.cover_letters (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    job_id uuid NOT NULL,
    resume_id uuid NOT NULL,
    profile_revision timestamp with time zone NOT NULL,
    job_revision timestamp with time zone NOT NULL,
    resume_revision timestamp with time zone NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT cover_letters_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

CREATE TABLE document.blobs (
    id uuid NOT NULL,
    sha256 text NOT NULL,
    byte_size bigint NOT NULL,
    content bytea NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT blobs_byte_size_matches_content CHECK ((byte_size = octet_length(content))),
    CONSTRAINT blobs_byte_size_positive CHECK ((byte_size > 0)),
    CONSTRAINT blobs_sha256_hex CHECK ((sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT blobs_sha256_not_blank CHECK ((btrim(sha256) <> ''::text))
);

CREATE TABLE document.documents (
    id uuid NOT NULL,
    blob_id uuid NOT NULL,
    original_file_name text NOT NULL,
    media_type text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT documents_media_type_not_blank CHECK ((btrim(media_type) <> ''::text)),
    CONSTRAINT documents_original_file_name_not_blank CHECK ((btrim(original_file_name) <> ''::text)),
    CONSTRAINT documents_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

CREATE TABLE document.generated_resume_file_cleanups (
    id uuid NOT NULL,
    file_path text NOT NULL,
    status text NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone NOT NULL,
    last_failure_type character varying(255),
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    completed_at timestamp with time zone,
    document_id uuid,
    CONSTRAINT generated_resume_file_cleanups_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT generated_resume_file_cleanups_status_check CHECK ((status = ANY (ARRAY['PENDING'::text, 'PROCESSING'::text, 'COMPLETED'::text])))
);

CREATE TABLE document.pdf_extractions (
    id uuid NOT NULL,
    file_id uuid NOT NULL,
    extractor text NOT NULL,
    character_count integer NOT NULL,
    page_count integer NOT NULL,
    truncated boolean NOT NULL,
    extracted_text text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    page_projections jsonb DEFAULT '[]'::jsonb NOT NULL,
    CONSTRAINT pdf_extractions_character_count_non_negative CHECK ((character_count >= 0)),
    CONSTRAINT pdf_extractions_extractor_not_blank CHECK ((btrim(extractor) <> ''::text)),
    CONSTRAINT pdf_extractions_page_count_positive CHECK ((page_count > 0)),
    CONSTRAINT pdf_extractions_page_projections_array CHECK ((jsonb_typeof(page_projections) = 'array'::text)),
    CONSTRAINT pdf_extractions_page_projections_bounded CHECK ((jsonb_array_length(page_projections) <= 200))
);

COMMENT ON COLUMN document.pdf_extractions.page_projections IS 'Bounded canonical page projections used to serve compatible cached extraction views.';

CREATE TABLE job_schema.job_link_ingestions (
    id uuid NOT NULL,
    job_id uuid NOT NULL,
    url text NOT NULL,
    normalized_url text NOT NULL,
    fetched_at timestamp with time zone NOT NULL,
    http_status integer,
    source_title text,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT job_link_ingestions_normalized_url_not_blank CHECK ((btrim(normalized_url) <> ''::text)),
    CONSTRAINT job_link_ingestions_normalized_url_safe_identity CHECK (((POSITION(('#'::text) IN (normalized_url)) = 0) AND (normalized_url !~* '^https?://[^/?#]*@'::text))),
    CONSTRAINT job_link_ingestions_status_valid CHECK (((http_status IS NULL) OR ((http_status >= 100) AND (http_status <= 599)))),
    CONSTRAINT job_link_ingestions_url_not_blank CHECK ((btrim(url) <> ''::text)),
    CONSTRAINT job_link_ingestions_url_safe_display CHECK (((POSITION(('?'::text) IN (url)) = 0) AND (POSITION(('#'::text) IN (url)) = 0) AND (url !~* '^https?://[^/?#]*@'::text)))
);

CREATE TABLE job_schema.job_skills (
    id uuid NOT NULL,
    job_id uuid NOT NULL,
    skill text NOT NULL,
    normalized_skill text NOT NULL,
    required boolean DEFAULT true NOT NULL,
    display_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT job_skills_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT job_skills_normalized_skill_not_blank CHECK ((btrim(normalized_skill) <> ''::text)),
    CONSTRAINT job_skills_skill_not_blank CHECK ((btrim(skill) <> ''::text))
);

CREATE TABLE job_schema.job_text_ingestions (
    id uuid NOT NULL,
    job_id uuid NOT NULL,
    source_label text,
    input_text_hash text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT job_text_ingestions_hash_not_blank CHECK ((btrim(input_text_hash) <> ''::text))
);

CREATE TABLE job_schema.jobs (
    id uuid NOT NULL,
    source_method text NOT NULL,
    source_label text,
    title text NOT NULL,
    company text,
    location text,
    description text NOT NULL,
    experience_requirement text,
    employment_type text,
    seniority text,
    posted_at timestamp with time zone,
    canonical_fingerprint text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT jobs_description_not_blank CHECK ((btrim(description) <> ''::text)),
    CONSTRAINT jobs_fingerprint_not_blank CHECK ((btrim(canonical_fingerprint) <> ''::text)),
    CONSTRAINT jobs_revision_non_negative CHECK ((revision >= 0)),
    CONSTRAINT jobs_source_method_known CHECK ((source_method = ANY (ARRAY['link'::text, 'text'::text]))),
    CONSTRAINT jobs_title_not_blank CHECK ((btrim(title) <> ''::text)),
    CONSTRAINT jobs_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

CREATE TABLE job_schema.search_terms (
    job_id uuid NOT NULL,
    field_key text NOT NULL,
    term text NOT NULL,
    weight smallint NOT NULL,
    CONSTRAINT search_terms_weight_check CHECK ((weight > 0))
);

CREATE TABLE match.disagreements (
    id uuid NOT NULL,
    fingerprint uuid NOT NULL,
    report_id uuid NOT NULL,
    review_id uuid NOT NULL,
    policy_version text NOT NULL,
    reasons jsonb NOT NULL,
    evidence_defect_codes jsonb DEFAULT '[]'::jsonb NOT NULL,
    status text NOT NULL,
    linear_issue_id text,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT disagreements_evidence_defect_codes_check CHECK (((jsonb_typeof(evidence_defect_codes) = 'array'::text) AND (evidence_defect_codes <@ '["structured_evidence_missing", "structured_evidence_incorrect", "requirement_provenance_missing", "outcome_calibration_issue"]'::jsonb))),
    CONSTRAINT disagreements_linear_issue_id_check CHECK (((linear_issue_id IS NULL) OR (btrim(linear_issue_id) <> ''::text))),
    CONSTRAINT disagreements_policy_version_check CHECK ((policy_version = 'divergence-v1'::text)),
    CONSTRAINT disagreements_reasons_check CHECK ((jsonb_typeof(reasons) = 'array'::text)),
    CONSTRAINT disagreements_status_check CHECK ((status = ANY (ARRAY['OPEN'::text, 'PENDING_ESCALATION'::text, 'ACKNOWLEDGED'::text, 'LINKED'::text])))
);

CREATE TABLE match.reports (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    job_id uuid NOT NULL,
    profile_revision timestamp with time zone NOT NULL,
    job_revision timestamp with time zone NOT NULL,
    algorithm_version text NOT NULL,
    overall_score integer NOT NULL,
    confidence integer NOT NULL,
    outcome text NOT NULL,
    blocker_mismatch boolean NOT NULL,
    components jsonb NOT NULL,
    evidence jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT reports_algorithm_version_check CHECK ((btrim(algorithm_version) <> ''::text)),
    CONSTRAINT reports_components_check CHECK ((jsonb_typeof(components) = 'array'::text)),
    CONSTRAINT reports_confidence_check CHECK (((confidence >= 0) AND (confidence <= 100))),
    CONSTRAINT reports_evidence_check CHECK ((jsonb_typeof(evidence) = 'array'::text)),
    CONSTRAINT reports_outcome_check CHECK ((outcome = ANY (ARRAY['STRONG_MATCH'::text, 'PARTIAL_MATCH'::text, 'WEAK_MATCH'::text, 'INSUFFICIENT_EVIDENCE'::text]))),
    CONSTRAINT reports_overall_score_check CHECK (((overall_score >= 0) AND (overall_score <= 100)))
);

CREATE TABLE match.reviews (
    id uuid NOT NULL,
    fingerprint uuid NOT NULL,
    report_id uuid NOT NULL,
    reviewer text NOT NULL,
    model text NOT NULL,
    review_version text NOT NULL,
    overall_score integer NOT NULL,
    outcome text NOT NULL,
    blocker_mismatch boolean NOT NULL,
    components jsonb NOT NULL,
    evidence jsonb NOT NULL,
    summary text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT reviews_components_check CHECK ((jsonb_typeof(components) = 'array'::text)),
    CONSTRAINT reviews_evidence_check CHECK ((jsonb_typeof(evidence) = 'array'::text)),
    CONSTRAINT reviews_model_check CHECK ((btrim(model) <> ''::text)),
    CONSTRAINT reviews_outcome_check CHECK ((outcome = ANY (ARRAY['STRONG_MATCH'::text, 'PARTIAL_MATCH'::text, 'WEAK_MATCH'::text, 'INSUFFICIENT_EVIDENCE'::text]))),
    CONSTRAINT reviews_overall_score_check CHECK (((overall_score >= 0) AND (overall_score <= 100))),
    CONSTRAINT reviews_review_version_check CHECK ((btrim(review_version) <> ''::text)),
    CONSTRAINT reviews_reviewer_check CHECK ((btrim(reviewer) <> ''::text)),
    CONSTRAINT reviews_summary_check CHECK ((summary = ANY (ARRAY['review_consistent'::text, 'score_adjustment'::text, 'outcome_adjustment'::text, 'evidence_defect_identified'::text])))
);

CREATE TABLE profile.education (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    institution text,
    degree text,
    field text,
    location text,
    start_date date,
    end_date date,
    description text,
    relevant_focus text,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT education_end_date_not_before_start_date CHECK (((start_date IS NULL) OR (end_date IS NULL) OR (end_date >= start_date)))
);

CREATE TABLE profile.experiences (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    company text,
    title text,
    location text,
    start_date date,
    end_date date,
    description text,
    display_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT experiences_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT experiences_end_date_not_before_start_date CHECK (((start_date IS NULL) OR (end_date IS NULL) OR (end_date >= start_date)))
);

CREATE TABLE profile.profile_contacts (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    contact_type text NOT NULL,
    contact_value text NOT NULL,
    label text,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT profile_contacts_type_not_blank CHECK ((btrim(contact_type) <> ''::text)),
    CONSTRAINT profile_contacts_updated_at_not_before_created_at CHECK ((updated_at >= created_at)),
    CONSTRAINT profile_contacts_value_not_blank CHECK ((btrim(contact_value) <> ''::text))
);

CREATE TABLE profile.profile_languages (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    language text NOT NULL,
    normalized_language text NOT NULL,
    proficiency text,
    display_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT profile_languages_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT profile_languages_language_not_blank CHECK ((btrim(language) <> ''::text)),
    CONSTRAINT profile_languages_normalized_language_not_blank CHECK ((btrim(normalized_language) <> ''::text))
);

CREATE TABLE profile.profile_links (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    link_type text NOT NULL,
    url text NOT NULL,
    label text,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT profile_links_type_not_blank CHECK ((btrim(link_type) <> ''::text)),
    CONSTRAINT profile_links_updated_at_not_before_created_at CHECK ((updated_at >= created_at)),
    CONSTRAINT profile_links_url_not_blank CHECK ((btrim(url) <> ''::text))
);

CREATE TABLE profile.profile_pdf_sources (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    pdf_extraction_id uuid NOT NULL,
    source_type text DEFAULT 'resume_pdf'::text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT profile_pdf_sources_source_type_not_blank CHECK ((btrim(source_type) <> ''::text))
);

CREATE TABLE profile.profile_personal_details (
    profile_id uuid NOT NULL,
    date_of_birth date,
    nationality text,
    photo_document_id uuid,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT profile_personal_details_nationality_not_blank CHECK (((nationality IS NULL) OR (btrim(nationality) <> ''::text))),
    CONSTRAINT profile_personal_details_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

CREATE TABLE profile.profile_resume_documents (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    document_id uuid NOT NULL,
    file_path text NOT NULL,
    resume_type text DEFAULT 'master_resume'::text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT profile_resume_documents_file_path_not_blank CHECK ((btrim(file_path) <> ''::text)),
    CONSTRAINT profile_resume_documents_resume_type_not_blank CHECK ((btrim(resume_type) <> ''::text)),
    CONSTRAINT profile_resume_documents_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

CREATE TABLE profile.profile_skills (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    skill text NOT NULL,
    normalized_skill text NOT NULL,
    category text,
    proficiency text,
    display_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT profile_skills_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT profile_skills_normalized_skill_not_blank CHECK ((btrim(normalized_skill) <> ''::text)),
    CONSTRAINT profile_skills_skill_not_blank CHECK ((btrim(skill) <> ''::text))
);

CREATE TABLE profile.profiles (
    id uuid NOT NULL,
    full_name text NOT NULL,
    email text NOT NULL,
    avatar_url text,
    summary text,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT profiles_email_not_blank CHECK ((btrim(email) <> ''::text)),
    CONSTRAINT profiles_full_name_not_blank CHECK ((btrim(full_name) <> ''::text)),
    CONSTRAINT profiles_revision_non_negative CHECK ((revision >= 0)),
    CONSTRAINT profiles_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

CREATE TABLE profile.project_technologies (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    technology text NOT NULL,
    normalized_technology text NOT NULL,
    display_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT project_technologies_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT project_technologies_normalized_technology_not_blank CHECK ((btrim(normalized_technology) <> ''::text)),
    CONSTRAINT project_technologies_technology_not_blank CHECK ((btrim(technology) <> ''::text))
);

CREATE TABLE profile.projects (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    name text,
    start_date date,
    end_date date,
    url text,
    description text,
    display_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT projects_display_order_non_negative CHECK ((display_order >= 0))
);

CREATE TABLE profile.search_terms (
    profile_id uuid NOT NULL,
    field_key text NOT NULL,
    term text NOT NULL,
    weight smallint NOT NULL,
    CONSTRAINT search_terms_weight_check CHECK ((weight > 0))
);

CREATE TABLE resume.resume_entries (
    id uuid NOT NULL,
    section_id uuid NOT NULL,
    entry_type text NOT NULL,
    display_order integer DEFAULT 0 NOT NULL,
    title text,
    organization text,
    location text,
    start_date date,
    end_date date,
    metadata text,
    CONSTRAINT resume_entries_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT resume_entries_entry_type_not_blank CHECK ((btrim(entry_type) <> ''::text))
);

CREATE TABLE resume.resume_entry_bullets (
    id uuid NOT NULL,
    entry_id uuid NOT NULL,
    display_order integer DEFAULT 0 NOT NULL,
    text text NOT NULL,
    CONSTRAINT resume_entry_bullets_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT resume_entry_bullets_text_not_blank CHECK ((btrim(text) <> ''::text))
);

CREATE TABLE resume.resume_sections (
    id uuid NOT NULL,
    variant_id uuid NOT NULL,
    section_type text NOT NULL,
    title text NOT NULL,
    display_order integer DEFAULT 0 NOT NULL,
    CONSTRAINT resume_sections_display_order_non_negative CHECK ((display_order >= 0)),
    CONSTRAINT resume_sections_section_type_not_blank CHECK ((btrim(section_type) <> ''::text)),
    CONSTRAINT resume_sections_title_not_blank CHECK ((btrim(title) <> ''::text))
);

CREATE TABLE resume.resume_variants (
    id uuid NOT NULL,
    resume_id uuid NOT NULL,
    language text NOT NULL,
    document_id uuid NOT NULL,
    file_path text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT resume_variants_file_path_not_blank CHECK ((btrim(file_path) <> ''::text)),
    CONSTRAINT resume_variants_language_known CHECK ((language = ANY (ARRAY['en'::text, 'de'::text]))),
    CONSTRAINT resume_variants_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

CREATE TABLE resume.resumes (
    id uuid NOT NULL,
    profile_id uuid NOT NULL,
    job_id uuid NOT NULL,
    format text NOT NULL,
    profile_revision timestamp with time zone NOT NULL,
    job_revision timestamp with time zone NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT resumes_format_known CHECK ((format = 'germany'::text)),
    CONSTRAINT resumes_format_not_blank CHECK ((btrim(format) <> ''::text)),
    CONSTRAINT resumes_updated_at_not_before_created_at CHECK ((updated_at >= created_at))
);

ALTER TABLE ONLY cover_letter.cover_letter_paragraphs
    ADD CONSTRAINT cover_letter_paragraphs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY cover_letter.cover_letter_paragraphs
    ADD CONSTRAINT cover_letter_paragraphs_variant_order_unique UNIQUE (variant_id, display_order);

ALTER TABLE ONLY cover_letter.cover_letter_variants
    ADD CONSTRAINT cover_letter_variants_cover_letter_format_language_unique UNIQUE (cover_letter_id, format, language);

ALTER TABLE ONLY cover_letter.cover_letter_variants
    ADD CONSTRAINT cover_letter_variants_document_id_unique UNIQUE (document_id);

ALTER TABLE ONLY cover_letter.cover_letter_variants
    ADD CONSTRAINT cover_letter_variants_pkey PRIMARY KEY (id);

ALTER TABLE ONLY cover_letter.cover_letters
    ADD CONSTRAINT cover_letters_pkey PRIMARY KEY (id);

ALTER TABLE ONLY cover_letter.cover_letters
    ADD CONSTRAINT cover_letters_profile_job_resume_unique UNIQUE (profile_id, job_id, resume_id);

ALTER TABLE ONLY document.blobs
    ADD CONSTRAINT blobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY document.documents
    ADD CONSTRAINT documents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY document.generated_resume_file_cleanups
    ADD CONSTRAINT generated_resume_file_cleanups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY document.pdf_extractions
    ADD CONSTRAINT pdf_extractions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY job_schema.job_link_ingestions
    ADD CONSTRAINT job_link_ingestions_job_id_key UNIQUE (job_id);

ALTER TABLE ONLY job_schema.job_link_ingestions
    ADD CONSTRAINT job_link_ingestions_normalized_url_unique UNIQUE (normalized_url);

ALTER TABLE ONLY job_schema.job_link_ingestions
    ADD CONSTRAINT job_link_ingestions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY job_schema.job_skills
    ADD CONSTRAINT job_skills_job_normalized_unique UNIQUE (job_id, normalized_skill);

ALTER TABLE ONLY job_schema.job_skills
    ADD CONSTRAINT job_skills_pkey PRIMARY KEY (id);

ALTER TABLE ONLY job_schema.job_text_ingestions
    ADD CONSTRAINT job_text_ingestions_hash_unique UNIQUE (input_text_hash);

ALTER TABLE ONLY job_schema.job_text_ingestions
    ADD CONSTRAINT job_text_ingestions_job_id_key UNIQUE (job_id);

ALTER TABLE ONLY job_schema.job_text_ingestions
    ADD CONSTRAINT job_text_ingestions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY job_schema.jobs
    ADD CONSTRAINT jobs_canonical_fingerprint_unique UNIQUE (canonical_fingerprint);

ALTER TABLE ONLY job_schema.jobs
    ADD CONSTRAINT jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY job_schema.search_terms
    ADD CONSTRAINT search_terms_pkey PRIMARY KEY (job_id, field_key, term);

ALTER TABLE ONLY match.disagreements
    ADD CONSTRAINT disagreements_fingerprint_key UNIQUE (fingerprint);

ALTER TABLE ONLY match.disagreements
    ADD CONSTRAINT disagreements_pkey PRIMARY KEY (id);

ALTER TABLE ONLY match.disagreements
    ADD CONSTRAINT disagreements_review_policy_unique UNIQUE (review_id, policy_version);

ALTER TABLE ONLY match.reports
    ADD CONSTRAINT reports_pkey PRIMARY KEY (id);

ALTER TABLE ONLY match.reports
    ADD CONSTRAINT reports_revision_unique UNIQUE (algorithm_version, profile_id, profile_revision, job_id, job_revision);

ALTER TABLE ONLY match.reviews
    ADD CONSTRAINT reviews_fingerprint_key UNIQUE (fingerprint);

ALTER TABLE ONLY match.reviews
    ADD CONSTRAINT reviews_id_report_unique UNIQUE (id, report_id);

ALTER TABLE ONLY match.reviews
    ADD CONSTRAINT reviews_pkey PRIMARY KEY (id);

ALTER TABLE profile.education
    ADD CONSTRAINT education_degree_canonical CHECK (((degree IS NULL) OR ((degree = btrim(degree)) AND (degree <> ''::text)))) NOT VALID;

ALTER TABLE profile.education
    ADD CONSTRAINT education_field_canonical CHECK (((field IS NULL) OR ((field = btrim(field)) AND (field <> ''::text)))) NOT VALID;

ALTER TABLE profile.education
    ADD CONSTRAINT education_institution_canonical CHECK (((institution IS NULL) OR ((institution = btrim(institution)) AND (institution <> ''::text)))) NOT VALID;

ALTER TABLE profile.education
    ADD CONSTRAINT education_location_canonical CHECK (((location IS NULL) OR ((location = btrim(location)) AND (location <> ''::text)))) NOT VALID;

ALTER TABLE ONLY profile.education
    ADD CONSTRAINT education_pkey PRIMARY KEY (id);

ALTER TABLE profile.education
    ADD CONSTRAINT education_relevant_focus_canonical CHECK (((relevant_focus IS NULL) OR ((relevant_focus = btrim(relevant_focus)) AND (relevant_focus <> ''::text)))) NOT VALID;

ALTER TABLE profile.experiences
    ADD CONSTRAINT experiences_company_canonical CHECK (((company IS NULL) OR ((company = btrim(company)) AND (company <> ''::text)))) NOT VALID;

ALTER TABLE profile.experiences
    ADD CONSTRAINT experiences_description_canonical CHECK (((description IS NULL) OR ((description = btrim(description)) AND (description <> ''::text)))) NOT VALID;

ALTER TABLE profile.experiences
    ADD CONSTRAINT experiences_location_canonical CHECK (((location IS NULL) OR ((location = btrim(location)) AND (location <> ''::text)))) NOT VALID;

ALTER TABLE ONLY profile.experiences
    ADD CONSTRAINT experiences_pkey PRIMARY KEY (id);

ALTER TABLE profile.experiences
    ADD CONSTRAINT experiences_title_canonical CHECK (((title IS NULL) OR ((title = btrim(title)) AND (title <> ''::text)))) NOT VALID;

ALTER TABLE profile.profile_contacts
    ADD CONSTRAINT profile_contacts_label_canonical CHECK (((label IS NULL) OR ((label = btrim(label)) AND (label <> ''::text)))) NOT VALID;

ALTER TABLE ONLY profile.profile_contacts
    ADD CONSTRAINT profile_contacts_pkey PRIMARY KEY (id);

ALTER TABLE profile.profile_contacts
    ADD CONSTRAINT profile_contacts_type_canonical CHECK ((contact_type = lower(btrim(contact_type)))) NOT VALID;

ALTER TABLE ONLY profile.profile_contacts
    ADD CONSTRAINT profile_contacts_type_value_unique UNIQUE (profile_id, contact_type, contact_value);

ALTER TABLE profile.profile_contacts
    ADD CONSTRAINT profile_contacts_value_canonical CHECK ((contact_value = btrim(contact_value))) NOT VALID;

ALTER TABLE profile.profile_languages
    ADD CONSTRAINT profile_languages_language_canonical CHECK ((language = btrim(language))) NOT VALID;

ALTER TABLE profile.profile_languages
    ADD CONSTRAINT profile_languages_normalized_language_canonical CHECK ((normalized_language = lower(btrim(normalized_language)))) NOT VALID;

ALTER TABLE ONLY profile.profile_languages
    ADD CONSTRAINT profile_languages_pkey PRIMARY KEY (id);

ALTER TABLE profile.profile_languages
    ADD CONSTRAINT profile_languages_proficiency_canonical CHECK (((proficiency IS NULL) OR ((proficiency = btrim(proficiency)) AND (proficiency <> ''::text)))) NOT VALID;

ALTER TABLE ONLY profile.profile_languages
    ADD CONSTRAINT profile_languages_profile_normalized_unique UNIQUE (profile_id, normalized_language);

ALTER TABLE profile.profile_links
    ADD CONSTRAINT profile_links_label_canonical CHECK (((label IS NULL) OR ((label = btrim(label)) AND (label <> ''::text)))) NOT VALID;

ALTER TABLE ONLY profile.profile_links
    ADD CONSTRAINT profile_links_pkey PRIMARY KEY (id);

ALTER TABLE profile.profile_links
    ADD CONSTRAINT profile_links_type_canonical CHECK ((link_type = lower(btrim(link_type)))) NOT VALID;

ALTER TABLE ONLY profile.profile_links
    ADD CONSTRAINT profile_links_type_url_unique UNIQUE (profile_id, link_type, url);

ALTER TABLE profile.profile_links
    ADD CONSTRAINT profile_links_url_canonical CHECK ((url = btrim(url))) NOT VALID;

ALTER TABLE ONLY profile.profile_pdf_sources
    ADD CONSTRAINT profile_pdf_sources_pkey PRIMARY KEY (id);

ALTER TABLE ONLY profile.profile_personal_details
    ADD CONSTRAINT profile_personal_details_pkey PRIMARY KEY (profile_id);

ALTER TABLE ONLY profile.profile_resume_documents
    ADD CONSTRAINT profile_resume_documents_pkey PRIMARY KEY (id);

ALTER TABLE profile.profile_skills
    ADD CONSTRAINT profile_skills_category_canonical CHECK (((category IS NULL) OR ((category = btrim(category)) AND (category <> ''::text)))) NOT VALID;

ALTER TABLE profile.profile_skills
    ADD CONSTRAINT profile_skills_normalized_skill_canonical CHECK ((normalized_skill = lower(btrim(normalized_skill)))) NOT VALID;

ALTER TABLE ONLY profile.profile_skills
    ADD CONSTRAINT profile_skills_pkey PRIMARY KEY (id);

ALTER TABLE ONLY profile.profile_skills
    ADD CONSTRAINT profile_skills_profile_normalized_unique UNIQUE (profile_id, normalized_skill);

ALTER TABLE profile.profile_skills
    ADD CONSTRAINT profile_skills_skill_canonical CHECK ((skill = btrim(skill))) NOT VALID;

ALTER TABLE profile.profiles
    ADD CONSTRAINT profiles_email_canonical CHECK ((email = lower(btrim(email)))) NOT VALID;

ALTER TABLE ONLY profile.profiles
    ADD CONSTRAINT profiles_email_unique UNIQUE (email);

ALTER TABLE ONLY profile.profiles
    ADD CONSTRAINT profiles_pkey PRIMARY KEY (id);

ALTER TABLE profile.project_technologies
    ADD CONSTRAINT project_technologies_normalized_technology_canonical CHECK ((normalized_technology = lower(btrim(normalized_technology)))) NOT VALID;

ALTER TABLE ONLY profile.project_technologies
    ADD CONSTRAINT project_technologies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY profile.project_technologies
    ADD CONSTRAINT project_technologies_project_normalized_unique UNIQUE (project_id, normalized_technology);

ALTER TABLE profile.project_technologies
    ADD CONSTRAINT project_technologies_technology_canonical CHECK ((technology = btrim(technology))) NOT VALID;

ALTER TABLE profile.projects
    ADD CONSTRAINT projects_description_canonical CHECK (((description IS NULL) OR ((description = btrim(description)) AND (description <> ''::text)))) NOT VALID;

ALTER TABLE profile.projects
    ADD CONSTRAINT projects_name_canonical CHECK (((name IS NULL) OR ((name = btrim(name)) AND (name <> ''::text)))) NOT VALID;

ALTER TABLE ONLY profile.projects
    ADD CONSTRAINT projects_pkey PRIMARY KEY (id);

ALTER TABLE profile.projects
    ADD CONSTRAINT projects_url_canonical CHECK (((url IS NULL) OR ((url = btrim(url)) AND (url <> ''::text)))) NOT VALID;

ALTER TABLE ONLY profile.search_terms
    ADD CONSTRAINT search_terms_pkey PRIMARY KEY (profile_id, field_key, term);

ALTER TABLE ONLY resume.resume_entries
    ADD CONSTRAINT resume_entries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resume.resume_entry_bullets
    ADD CONSTRAINT resume_entry_bullets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resume.resume_sections
    ADD CONSTRAINT resume_sections_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resume.resume_variants
    ADD CONSTRAINT resume_variants_document_id_unique UNIQUE (document_id);

ALTER TABLE ONLY resume.resume_variants
    ADD CONSTRAINT resume_variants_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resume.resume_variants
    ADD CONSTRAINT resume_variants_resume_language_unique UNIQUE (resume_id, language);

ALTER TABLE ONLY resume.resumes
    ADD CONSTRAINT resumes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resume.resumes
    ADD CONSTRAINT resumes_profile_job_format_unique UNIQUE (profile_id, job_id, format);

CREATE INDEX idx_cover_letter_paragraphs_variant_order ON cover_letter.cover_letter_paragraphs USING btree (variant_id, display_order);

CREATE INDEX idx_cover_letter_variants_cover_letter_id ON cover_letter.cover_letter_variants USING btree (cover_letter_id);

CREATE INDEX idx_cover_letters_job_id ON cover_letter.cover_letters USING btree (job_id);

CREATE INDEX idx_cover_letters_profile_id ON cover_letter.cover_letters USING btree (profile_id);

CREATE INDEX idx_cover_letters_resume_id ON cover_letter.cover_letters USING btree (resume_id);

CREATE INDEX idx_document_blobs_created_at ON document.blobs USING btree (created_at DESC, id);

CREATE UNIQUE INDEX idx_document_blobs_sha256_unique ON document.blobs USING btree (sha256);

CREATE INDEX idx_document_documents_blob_id ON document.documents USING btree (blob_id);

CREATE INDEX idx_document_documents_created_at ON document.documents USING btree (created_at DESC, id);

CREATE INDEX idx_document_documents_media_type ON document.documents USING btree (media_type);

CREATE INDEX idx_generated_resume_file_cleanups_completed_retention ON document.generated_resume_file_cleanups USING btree (completed_at, id) WHERE (status = 'COMPLETED'::text);

CREATE INDEX idx_generated_resume_file_cleanups_due ON document.generated_resume_file_cleanups USING btree (next_attempt_at, created_at) WHERE (status = ANY (ARRAY['PENDING'::text, 'PROCESSING'::text]));

CREATE INDEX idx_pdf_extractions_file_created_at ON document.pdf_extractions USING btree (file_id, created_at DESC, id);

CREATE UNIQUE INDEX idx_pdf_extractions_file_id_unique ON document.pdf_extractions USING btree (file_id);

CREATE INDEX idx_job_link_ingestions_job_id ON job_schema.job_link_ingestions USING btree (job_id);

CREATE INDEX idx_job_skills_job_order ON job_schema.job_skills USING btree (job_id, display_order);

CREATE INDEX idx_job_skills_normalized ON job_schema.job_skills USING btree (normalized_skill);

CREATE INDEX idx_job_text_ingestions_job_id ON job_schema.job_text_ingestions USING btree (job_id);

CREATE INDEX idx_jobs_company ON job_schema.jobs USING btree (company);

CREATE INDEX idx_jobs_posted_at ON job_schema.jobs USING btree (posted_at DESC NULLS LAST);

CREATE INDEX idx_jobs_source_method ON job_schema.jobs USING btree (source_method);

CREATE INDEX idx_jobs_title ON job_schema.jobs USING btree (title);

CREATE INDEX job_search_terms_term_prefix_idx ON job_schema.search_terms USING btree (term COLLATE "C", job_id, field_key) INCLUDE (weight);

CREATE INDEX jobs_list_created_id_idx ON job_schema.jobs USING btree (created_at DESC, id);

CREATE INDEX disagreements_list_created_id_idx ON match.disagreements USING btree (created_at DESC, id DESC);

CREATE INDEX disagreements_report_created_id_idx ON match.disagreements USING btree (report_id, created_at DESC, id DESC);

CREATE INDEX reports_job_created_id_idx ON match.reports USING btree (job_id, created_at DESC, id DESC);

CREATE INDEX reports_list_created_id_idx ON match.reports USING btree (created_at DESC, id DESC);

CREATE INDEX reports_profile_created_id_idx ON match.reports USING btree (profile_id, created_at DESC, id DESC);

CREATE INDEX reports_profile_job_created_id_idx ON match.reports USING btree ((ARRAY[profile_id, job_id]), created_at DESC, id DESC);

CREATE INDEX reviews_list_created_id_idx ON match.reviews USING btree (created_at DESC, id DESC);

CREATE INDEX reviews_report_created_id_idx ON match.reviews USING btree (report_id, created_at DESC, id DESC);

CREATE UNIQUE INDEX idx_education_canonical_unique ON profile.education USING btree (profile_id, COALESCE(lower(btrim(institution)), ''::text), COALESCE(lower(btrim(degree)), ''::text), COALESCE(lower(btrim(field)), ''::text), COALESCE(start_date, '0001-01-01'::date), COALESCE(end_date, '9999-12-31'::date));

CREATE INDEX idx_education_profile_id ON profile.education USING btree (profile_id);

CREATE UNIQUE INDEX idx_experiences_canonical_unique ON profile.experiences USING btree (profile_id, COALESCE(lower(btrim(company)), ''::text), COALESCE(lower(btrim(title)), ''::text), COALESCE(start_date, '0001-01-01'::date), COALESCE(end_date, '9999-12-31'::date));

CREATE INDEX idx_experiences_company ON profile.experiences USING btree (company);

CREATE INDEX idx_experiences_profile_order ON profile.experiences USING btree (profile_id, display_order);

CREATE INDEX idx_profile_contacts_profile_id ON profile.profile_contacts USING btree (profile_id);

CREATE INDEX idx_profile_contacts_type ON profile.profile_contacts USING btree (contact_type);

CREATE UNIQUE INDEX idx_profile_contacts_type_value_canonical_unique ON profile.profile_contacts USING btree (profile_id, lower(btrim(contact_type)), btrim(contact_value));

CREATE INDEX idx_profile_languages_normalized ON profile.profile_languages USING btree (normalized_language);

CREATE UNIQUE INDEX idx_profile_languages_normalized_canonical_unique ON profile.profile_languages USING btree (profile_id, lower(btrim(normalized_language)));

CREATE INDEX idx_profile_languages_profile_order ON profile.profile_languages USING btree (profile_id, display_order);

CREATE INDEX idx_profile_links_profile_id ON profile.profile_links USING btree (profile_id);

CREATE INDEX idx_profile_links_type ON profile.profile_links USING btree (link_type);

CREATE UNIQUE INDEX idx_profile_links_type_url_canonical_unique ON profile.profile_links USING btree (profile_id, lower(btrim(link_type)), btrim(url));

CREATE INDEX idx_profile_pdf_sources_created_at ON profile.profile_pdf_sources USING btree (created_at DESC, id);

CREATE UNIQUE INDEX idx_profile_pdf_sources_pdf_extraction_id_unique ON profile.profile_pdf_sources USING btree (pdf_extraction_id);

CREATE UNIQUE INDEX idx_profile_pdf_sources_profile_id_unique ON profile.profile_pdf_sources USING btree (profile_id);

CREATE INDEX idx_profile_personal_details_photo_document_id ON profile.profile_personal_details USING btree (photo_document_id) WHERE (photo_document_id IS NOT NULL);

CREATE UNIQUE INDEX idx_profile_resume_documents_document_id_unique ON profile.profile_resume_documents USING btree (document_id);

CREATE UNIQUE INDEX idx_profile_resume_documents_file_path_unique ON profile.profile_resume_documents USING btree (file_path);

CREATE UNIQUE INDEX idx_profile_resume_documents_profile_id_resume_type_unique ON profile.profile_resume_documents USING btree (profile_id, resume_type);

CREATE INDEX idx_profile_resume_documents_updated_at ON profile.profile_resume_documents USING btree (updated_at DESC, id);

CREATE INDEX idx_profile_skills_normalized ON profile.profile_skills USING btree (normalized_skill);

CREATE UNIQUE INDEX idx_profile_skills_normalized_canonical_unique ON profile.profile_skills USING btree (profile_id, lower(btrim(normalized_skill)));

CREATE INDEX idx_profile_skills_profile_order ON profile.profile_skills USING btree (profile_id, display_order);

CREATE UNIQUE INDEX idx_profiles_email_canonical_unique ON profile.profiles USING btree (lower(btrim(email)));

CREATE INDEX idx_profiles_full_name ON profile.profiles USING btree (full_name);

CREATE INDEX idx_project_technologies_normalized ON profile.project_technologies USING btree (normalized_technology);

CREATE UNIQUE INDEX idx_project_technologies_normalized_canonical_unique ON profile.project_technologies USING btree (project_id, lower(btrim(normalized_technology)));

CREATE INDEX idx_project_technologies_project_order ON profile.project_technologies USING btree (project_id, display_order);

CREATE UNIQUE INDEX idx_projects_canonical_unique ON profile.projects USING btree (profile_id, COALESCE(lower(btrim(name)), ''::text), COALESCE(btrim(url), ''::text));

CREATE INDEX idx_projects_profile_order ON profile.projects USING btree (profile_id, display_order);

CREATE INDEX profile_search_terms_term_prefix_idx ON profile.search_terms USING btree (term COLLATE "C", profile_id, field_key) INCLUDE (weight);

CREATE INDEX profiles_list_created_id_idx ON profile.profiles USING btree (created_at DESC, id);

CREATE INDEX idx_resume_entries_section_order ON resume.resume_entries USING btree (section_id, display_order);

CREATE INDEX idx_resume_entry_bullets_entry_order ON resume.resume_entry_bullets USING btree (entry_id, display_order);

CREATE INDEX idx_resume_sections_variant_order ON resume.resume_sections USING btree (variant_id, display_order);

CREATE INDEX idx_resume_variants_resume_id ON resume.resume_variants USING btree (resume_id);

CREATE INDEX idx_resumes_job_id ON resume.resumes USING btree (job_id);

CREATE INDEX idx_resumes_profile_id ON resume.resumes USING btree (profile_id);

CREATE INDEX idx_resumes_updated_at ON resume.resumes USING btree (updated_at DESC, id);

CREATE UNIQUE INDEX resumes_profile_job_id_unique ON resume.resumes USING btree (profile_id, job_id, id);

CREATE CONSTRAINT TRIGGER job_link_ingestions_source_provenance_enforcement AFTER INSERT OR DELETE OR UPDATE ON job_schema.job_link_ingestions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION job_schema.enforce_job_source_provenance();

CREATE CONSTRAINT TRIGGER job_text_ingestions_source_provenance_enforcement AFTER INSERT OR DELETE OR UPDATE ON job_schema.job_text_ingestions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION job_schema.enforce_job_source_provenance();

CREATE CONSTRAINT TRIGGER jobs_source_provenance_enforcement AFTER INSERT OR UPDATE OF source_method ON job_schema.jobs DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION job_schema.enforce_job_source_provenance();

CREATE TRIGGER reports_immutable BEFORE UPDATE ON match.reports FOR EACH ROW EXECUTE FUNCTION match.reject_immutable_update();

CREATE TRIGGER reviews_immutable BEFORE UPDATE ON match.reviews FOR EACH ROW EXECUTE FUNCTION match.reject_immutable_update();

ALTER TABLE ONLY cover_letter.cover_letter_paragraphs
    ADD CONSTRAINT cover_letter_paragraphs_variant_id_fk FOREIGN KEY (variant_id) REFERENCES cover_letter.cover_letter_variants(id) ON DELETE CASCADE;

ALTER TABLE ONLY cover_letter.cover_letter_variants
    ADD CONSTRAINT cover_letter_variants_cover_letter_id_fk FOREIGN KEY (cover_letter_id) REFERENCES cover_letter.cover_letters(id) ON DELETE CASCADE;

ALTER TABLE ONLY cover_letter.cover_letter_variants
    ADD CONSTRAINT cover_letter_variants_document_id_fk FOREIGN KEY (document_id) REFERENCES document.documents(id) ON DELETE RESTRICT;

ALTER TABLE ONLY cover_letter.cover_letters
    ADD CONSTRAINT cover_letters_job_id_fk FOREIGN KEY (job_id) REFERENCES job_schema.jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY cover_letter.cover_letters
    ADD CONSTRAINT cover_letters_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY cover_letter.cover_letters
    ADD CONSTRAINT cover_letters_resume_identity_fk FOREIGN KEY (profile_id, job_id, resume_id) REFERENCES resume.resumes(profile_id, job_id, id) ON DELETE CASCADE;

ALTER TABLE ONLY document.documents
    ADD CONSTRAINT documents_blob_id_fk FOREIGN KEY (blob_id) REFERENCES document.blobs(id) ON DELETE RESTRICT;

ALTER TABLE ONLY document.pdf_extractions
    ADD CONSTRAINT pdf_extractions_file_id_fk FOREIGN KEY (file_id) REFERENCES document.documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY job_schema.job_link_ingestions
    ADD CONSTRAINT job_link_ingestions_job_id_fk FOREIGN KEY (job_id) REFERENCES job_schema.jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY job_schema.job_skills
    ADD CONSTRAINT job_skills_job_id_fk FOREIGN KEY (job_id) REFERENCES job_schema.jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY job_schema.job_text_ingestions
    ADD CONSTRAINT job_text_ingestions_job_id_fk FOREIGN KEY (job_id) REFERENCES job_schema.jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY job_schema.search_terms
    ADD CONSTRAINT search_terms_job_id_fkey FOREIGN KEY (job_id) REFERENCES job_schema.jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY match.disagreements
    ADD CONSTRAINT disagreements_report_id_fkey FOREIGN KEY (report_id) REFERENCES match.reports(id) ON DELETE CASCADE;

ALTER TABLE ONLY match.disagreements
    ADD CONSTRAINT disagreements_review_id_fkey FOREIGN KEY (review_id) REFERENCES match.reviews(id) ON DELETE CASCADE;

ALTER TABLE ONLY match.disagreements
    ADD CONSTRAINT disagreements_review_report_fk FOREIGN KEY (review_id, report_id) REFERENCES match.reviews(id, report_id) ON DELETE CASCADE;

ALTER TABLE ONLY match.reports
    ADD CONSTRAINT reports_job_id_fkey FOREIGN KEY (job_id) REFERENCES job_schema.jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY match.reports
    ADD CONSTRAINT reports_profile_id_fkey FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY match.reviews
    ADD CONSTRAINT reviews_report_id_fkey FOREIGN KEY (report_id) REFERENCES match.reports(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.education
    ADD CONSTRAINT education_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.experiences
    ADD CONSTRAINT experiences_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.profile_contacts
    ADD CONSTRAINT profile_contacts_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.profile_languages
    ADD CONSTRAINT profile_languages_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.profile_links
    ADD CONSTRAINT profile_links_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.profile_pdf_sources
    ADD CONSTRAINT profile_pdf_sources_pdf_extraction_id_fk FOREIGN KEY (pdf_extraction_id) REFERENCES document.pdf_extractions(id) ON DELETE RESTRICT;

ALTER TABLE ONLY profile.profile_pdf_sources
    ADD CONSTRAINT profile_pdf_sources_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.profile_personal_details
    ADD CONSTRAINT profile_personal_details_photo_document_id_fk FOREIGN KEY (photo_document_id) REFERENCES document.documents(id) ON DELETE SET NULL;

ALTER TABLE ONLY profile.profile_personal_details
    ADD CONSTRAINT profile_personal_details_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.profile_resume_documents
    ADD CONSTRAINT profile_resume_documents_document_id_fk FOREIGN KEY (document_id) REFERENCES document.documents(id) ON DELETE RESTRICT;

ALTER TABLE ONLY profile.profile_resume_documents
    ADD CONSTRAINT profile_resume_documents_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.profile_skills
    ADD CONSTRAINT profile_skills_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.project_technologies
    ADD CONSTRAINT project_technologies_project_id_fk FOREIGN KEY (project_id) REFERENCES profile.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.projects
    ADD CONSTRAINT projects_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY profile.search_terms
    ADD CONSTRAINT search_terms_profile_id_fkey FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY resume.resume_entries
    ADD CONSTRAINT resume_entries_section_id_fk FOREIGN KEY (section_id) REFERENCES resume.resume_sections(id) ON DELETE CASCADE;

ALTER TABLE ONLY resume.resume_entry_bullets
    ADD CONSTRAINT resume_entry_bullets_entry_id_fk FOREIGN KEY (entry_id) REFERENCES resume.resume_entries(id) ON DELETE CASCADE;

ALTER TABLE ONLY resume.resume_sections
    ADD CONSTRAINT resume_sections_variant_id_fk FOREIGN KEY (variant_id) REFERENCES resume.resume_variants(id) ON DELETE CASCADE;

ALTER TABLE ONLY resume.resume_variants
    ADD CONSTRAINT resume_variants_document_id_fk FOREIGN KEY (document_id) REFERENCES document.documents(id) ON DELETE RESTRICT;

ALTER TABLE ONLY resume.resume_variants
    ADD CONSTRAINT resume_variants_resume_id_fk FOREIGN KEY (resume_id) REFERENCES resume.resumes(id) ON DELETE CASCADE;

ALTER TABLE ONLY resume.resumes
    ADD CONSTRAINT resumes_job_id_fk FOREIGN KEY (job_id) REFERENCES job_schema.jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY resume.resumes
    ADD CONSTRAINT resumes_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile.profiles(id) ON DELETE CASCADE;
