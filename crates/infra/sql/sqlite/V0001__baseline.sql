-- Jobniac SQLite baseline schema (V0001).
--
-- Equivalent to the PostgreSQL migrations V0001 to V0006 (the Spring baseline, the resume section
-- order constraint and the owned generated-asset tables). No PostgreSQL history is replayed: the
-- V0002 import ledger and the V0004 search-posting rewrite do not exist here, because SQLite
-- starts empty.
--
-- Conventions:
-- * Schemas become table-name prefixes, always `<prefix>_<table>`: profile, job (job_schema),
--   match, document, resume, cover_letter, jobniac. For example `profile.profiles` is
--   `profile_profiles`, `job_schema.job_skills` is `job_job_skills`.
-- * Every table is STRICT. uuid is TEXT (36 lowercase hex and dash characters, checked on the
--   primary keys and on stand-alone uuid columns; foreign keys inherit the check from their
--   parent). timestamptz is TEXT, fixed-width RFC 3339 UTC written by Rust (string order is time
--   order only while the width is fixed). date is TEXT `YYYY-MM-DD`. boolean is INTEGER 0 or 1.
--   jsonb is TEXT with json_valid. bytea is BLOB. bigint and smallint are INTEGER.
-- * `lower()` is ASCII-only in SQLite, so the canonical-form CHECKs and the canonical unique
--   indexes only cover ASCII. Rust normalizes with full Unicode lowercase before every write.
-- * Rules that need a sub-query, a regular expression or advisory locks are not in this file.
--   They are listed in RULES_TO_PORT.md next to the Rust adapter and are enforced there. Pure
--   row-local immutability rules are triggers that RAISE(ABORT, ...).
-- * `row_version` of jobniac_generated_assets replaces the PostgreSQL `xmin` freshness read.

CREATE TABLE document_blobs (
    id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    byte_size INTEGER NOT NULL,
    content BLOB NOT NULL,
    created_at TEXT NOT NULL,
    CONSTRAINT blobs_pkey PRIMARY KEY (id),
    CONSTRAINT blobs_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT blobs_byte_size_matches_content CHECK (byte_size = length(content)),
    CONSTRAINT blobs_byte_size_positive CHECK (byte_size > 0),
    CONSTRAINT blobs_sha256_hex CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    CONSTRAINT blobs_sha256_not_blank CHECK (trim(sha256) <> '')
) STRICT;

CREATE TABLE document_documents (
    id TEXT NOT NULL,
    blob_id TEXT NOT NULL,
    original_file_name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT documents_pkey PRIMARY KEY (id),
    CONSTRAINT documents_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT documents_blob_id_fk FOREIGN KEY (blob_id) REFERENCES document_blobs(id) ON DELETE RESTRICT,
    CONSTRAINT documents_media_type_not_blank CHECK (trim(media_type) <> ''),
    CONSTRAINT documents_original_file_name_not_blank CHECK (trim(original_file_name) <> ''),
    CONSTRAINT documents_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE document_generated_resume_file_cleanups (
    id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    last_failure_type TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    document_id TEXT,
    CONSTRAINT generated_resume_file_cleanups_pkey PRIMARY KEY (id),
    CONSTRAINT generated_resume_file_cleanups_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT generated_resume_file_cleanups_attempt_count_check CHECK (attempt_count >= 0),
    CONSTRAINT generated_resume_file_cleanups_status_check CHECK (status IN ('PENDING', 'PROCESSING', 'COMPLETED')),
    CONSTRAINT generated_resume_file_cleanups_failure_type_length CHECK (last_failure_type IS NULL OR length(last_failure_type) <= 255)
) STRICT;

CREATE TABLE document_pdf_extractions (
    id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    extractor TEXT NOT NULL,
    character_count INTEGER NOT NULL,
    page_count INTEGER NOT NULL,
    truncated INTEGER NOT NULL,
    extracted_text TEXT NOT NULL,
    created_at TEXT NOT NULL,
    -- Bounded canonical page projections used to serve compatible cached extraction views.
    page_projections TEXT NOT NULL DEFAULT '[]',
    CONSTRAINT pdf_extractions_pkey PRIMARY KEY (id),
    CONSTRAINT pdf_extractions_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT pdf_extractions_file_id_fk FOREIGN KEY (file_id) REFERENCES document_documents(id) ON DELETE CASCADE,
    CONSTRAINT pdf_extractions_character_count_non_negative CHECK (character_count >= 0),
    CONSTRAINT pdf_extractions_extractor_not_blank CHECK (trim(extractor) <> ''),
    CONSTRAINT pdf_extractions_page_count_positive CHECK (page_count > 0),
    CONSTRAINT pdf_extractions_truncated_bool CHECK (truncated IN (0, 1)),
    CONSTRAINT pdf_extractions_page_projections_array CHECK (CASE WHEN json_valid(page_projections) THEN json_type(page_projections) = 'array' ELSE 0 END),
    CONSTRAINT pdf_extractions_page_projections_bounded CHECK (CASE WHEN json_valid(page_projections) THEN json_array_length(page_projections) <= 200 ELSE 0 END)
) STRICT;

CREATE TABLE job_jobs (
    id TEXT NOT NULL,
    source_method TEXT NOT NULL,
    source_label TEXT,
    title TEXT NOT NULL,
    company TEXT,
    location TEXT,
    description TEXT NOT NULL,
    experience_requirement TEXT,
    employment_type TEXT,
    seniority TEXT,
    posted_at TEXT,
    canonical_fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0,
    CONSTRAINT jobs_pkey PRIMARY KEY (id),
    CONSTRAINT jobs_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT jobs_canonical_fingerprint_unique UNIQUE (canonical_fingerprint),
    CONSTRAINT jobs_description_not_blank CHECK (trim(description) <> ''),
    CONSTRAINT jobs_fingerprint_not_blank CHECK (trim(canonical_fingerprint) <> ''),
    CONSTRAINT jobs_revision_non_negative CHECK (revision >= 0),
    CONSTRAINT jobs_source_method_known CHECK (source_method IN ('link', 'text')),
    CONSTRAINT jobs_title_not_blank CHECK (trim(title) <> ''),
    CONSTRAINT jobs_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE job_job_link_ingestions (
    id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    url TEXT NOT NULL,
    normalized_url TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    http_status INTEGER,
    source_title TEXT,
    created_at TEXT NOT NULL,
    CONSTRAINT job_link_ingestions_pkey PRIMARY KEY (id),
    CONSTRAINT job_link_ingestions_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT job_link_ingestions_job_id_key UNIQUE (job_id),
    CONSTRAINT job_link_ingestions_normalized_url_unique UNIQUE (normalized_url),
    CONSTRAINT job_link_ingestions_job_id_fk FOREIGN KEY (job_id) REFERENCES job_jobs(id) ON DELETE CASCADE,
    CONSTRAINT job_link_ingestions_normalized_url_not_blank CHECK (trim(normalized_url) <> ''),
    -- The userinfo half of the PostgreSQL check is a regular expression: see RULES_TO_PORT.md.
    CONSTRAINT job_link_ingestions_normalized_url_safe_identity CHECK (instr(normalized_url, '#') = 0),
    CONSTRAINT job_link_ingestions_status_valid CHECK (http_status IS NULL OR (http_status >= 100 AND http_status <= 599)),
    CONSTRAINT job_link_ingestions_url_not_blank CHECK (trim(url) <> ''),
    CONSTRAINT job_link_ingestions_url_safe_display CHECK (instr(url, '?') = 0 AND instr(url, '#') = 0)
) STRICT;

CREATE TABLE job_job_skills (
    id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    skill TEXT NOT NULL,
    normalized_skill TEXT NOT NULL,
    required INTEGER NOT NULL DEFAULT 1,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    CONSTRAINT job_skills_pkey PRIMARY KEY (id),
    CONSTRAINT job_skills_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT job_skills_job_normalized_unique UNIQUE (job_id, normalized_skill),
    CONSTRAINT job_skills_job_id_fk FOREIGN KEY (job_id) REFERENCES job_jobs(id) ON DELETE CASCADE,
    CONSTRAINT job_skills_required_bool CHECK (required IN (0, 1)),
    CONSTRAINT job_skills_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT job_skills_normalized_skill_not_blank CHECK (trim(normalized_skill) <> ''),
    CONSTRAINT job_skills_skill_not_blank CHECK (trim(skill) <> '')
) STRICT;

CREATE TABLE job_job_text_ingestions (
    id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    source_label TEXT,
    input_text_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CONSTRAINT job_text_ingestions_pkey PRIMARY KEY (id),
    CONSTRAINT job_text_ingestions_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT job_text_ingestions_hash_unique UNIQUE (input_text_hash),
    CONSTRAINT job_text_ingestions_job_id_key UNIQUE (job_id),
    CONSTRAINT job_text_ingestions_job_id_fk FOREIGN KEY (job_id) REFERENCES job_jobs(id) ON DELETE CASCADE,
    CONSTRAINT job_text_ingestions_hash_not_blank CHECK (trim(input_text_hash) <> '')
) STRICT;

CREATE TABLE job_search_terms (
    job_id TEXT NOT NULL,
    field_key TEXT NOT NULL,
    term TEXT NOT NULL,
    weight INTEGER NOT NULL,
    CONSTRAINT search_terms_pkey PRIMARY KEY (job_id, field_key, term),
    CONSTRAINT search_terms_job_id_fkey FOREIGN KEY (job_id) REFERENCES job_jobs(id) ON DELETE CASCADE,
    CONSTRAINT search_terms_weight_check CHECK (weight > 0 AND weight <= 32767)
) STRICT;

CREATE TABLE profile_profiles (
    id TEXT NOT NULL,
    full_name TEXT NOT NULL,
    email TEXT NOT NULL,
    avatar_url TEXT,
    summary TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0,
    CONSTRAINT profiles_pkey PRIMARY KEY (id),
    CONSTRAINT profiles_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT profiles_email_unique UNIQUE (email),
    CONSTRAINT profiles_email_canonical CHECK (email = lower(trim(email))),
    CONSTRAINT profiles_email_not_blank CHECK (trim(email) <> ''),
    CONSTRAINT profiles_full_name_not_blank CHECK (trim(full_name) <> ''),
    CONSTRAINT profiles_revision_non_negative CHECK (revision >= 0),
    CONSTRAINT profiles_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE profile_education (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    institution TEXT,
    degree TEXT,
    field TEXT,
    location TEXT,
    start_date TEXT,
    end_date TEXT,
    description TEXT,
    relevant_focus TEXT,
    created_at TEXT NOT NULL,
    CONSTRAINT education_pkey PRIMARY KEY (id),
    CONSTRAINT education_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT education_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT education_start_date_valid CHECK (start_date IS NULL OR date(start_date) = start_date),
    CONSTRAINT education_end_date_valid CHECK (end_date IS NULL OR date(end_date) = end_date),
    CONSTRAINT education_end_date_not_before_start_date CHECK (start_date IS NULL OR end_date IS NULL OR end_date >= start_date),
    CONSTRAINT education_degree_canonical CHECK (degree IS NULL OR (degree = trim(degree) AND degree <> '')),
    CONSTRAINT education_field_canonical CHECK (field IS NULL OR (field = trim(field) AND field <> '')),
    CONSTRAINT education_institution_canonical CHECK (institution IS NULL OR (institution = trim(institution) AND institution <> '')),
    CONSTRAINT education_location_canonical CHECK (location IS NULL OR (location = trim(location) AND location <> '')),
    CONSTRAINT education_relevant_focus_canonical CHECK (relevant_focus IS NULL OR (relevant_focus = trim(relevant_focus) AND relevant_focus <> ''))
) STRICT;

CREATE TABLE profile_experiences (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    company TEXT,
    title TEXT,
    location TEXT,
    start_date TEXT,
    end_date TEXT,
    description TEXT,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    CONSTRAINT experiences_pkey PRIMARY KEY (id),
    CONSTRAINT experiences_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT experiences_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT experiences_start_date_valid CHECK (start_date IS NULL OR date(start_date) = start_date),
    CONSTRAINT experiences_end_date_valid CHECK (end_date IS NULL OR date(end_date) = end_date),
    CONSTRAINT experiences_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT experiences_end_date_not_before_start_date CHECK (start_date IS NULL OR end_date IS NULL OR end_date >= start_date),
    CONSTRAINT experiences_company_canonical CHECK (company IS NULL OR (company = trim(company) AND company <> '')),
    CONSTRAINT experiences_description_canonical CHECK (description IS NULL OR (description = trim(description) AND description <> '')),
    CONSTRAINT experiences_location_canonical CHECK (location IS NULL OR (location = trim(location) AND location <> '')),
    CONSTRAINT experiences_title_canonical CHECK (title IS NULL OR (title = trim(title) AND title <> ''))
) STRICT;

CREATE TABLE profile_profile_contacts (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    contact_type TEXT NOT NULL,
    contact_value TEXT NOT NULL,
    label TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT profile_contacts_pkey PRIMARY KEY (id),
    CONSTRAINT profile_contacts_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT profile_contacts_type_value_unique UNIQUE (profile_id, contact_type, contact_value),
    CONSTRAINT profile_contacts_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT profile_contacts_type_not_blank CHECK (trim(contact_type) <> ''),
    CONSTRAINT profile_contacts_updated_at_not_before_created_at CHECK (updated_at >= created_at),
    CONSTRAINT profile_contacts_value_not_blank CHECK (trim(contact_value) <> ''),
    CONSTRAINT profile_contacts_label_canonical CHECK (label IS NULL OR (label = trim(label) AND label <> '')),
    CONSTRAINT profile_contacts_type_canonical CHECK (contact_type = lower(trim(contact_type))),
    CONSTRAINT profile_contacts_value_canonical CHECK (contact_value = trim(contact_value))
) STRICT;

CREATE TABLE profile_profile_languages (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    language TEXT NOT NULL,
    normalized_language TEXT NOT NULL,
    proficiency TEXT,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    CONSTRAINT profile_languages_pkey PRIMARY KEY (id),
    CONSTRAINT profile_languages_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT profile_languages_profile_normalized_unique UNIQUE (profile_id, normalized_language),
    CONSTRAINT profile_languages_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT profile_languages_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT profile_languages_language_not_blank CHECK (trim(language) <> ''),
    CONSTRAINT profile_languages_normalized_language_not_blank CHECK (trim(normalized_language) <> ''),
    CONSTRAINT profile_languages_language_canonical CHECK (language = trim(language)),
    CONSTRAINT profile_languages_normalized_language_canonical CHECK (normalized_language = lower(trim(normalized_language))),
    CONSTRAINT profile_languages_proficiency_canonical CHECK (proficiency IS NULL OR (proficiency = trim(proficiency) AND proficiency <> ''))
) STRICT;

CREATE TABLE profile_profile_links (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    link_type TEXT NOT NULL,
    url TEXT NOT NULL,
    label TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT profile_links_pkey PRIMARY KEY (id),
    CONSTRAINT profile_links_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT profile_links_type_url_unique UNIQUE (profile_id, link_type, url),
    CONSTRAINT profile_links_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT profile_links_type_not_blank CHECK (trim(link_type) <> ''),
    CONSTRAINT profile_links_updated_at_not_before_created_at CHECK (updated_at >= created_at),
    CONSTRAINT profile_links_url_not_blank CHECK (trim(url) <> ''),
    CONSTRAINT profile_links_label_canonical CHECK (label IS NULL OR (label = trim(label) AND label <> '')),
    CONSTRAINT profile_links_type_canonical CHECK (link_type = lower(trim(link_type))),
    CONSTRAINT profile_links_url_canonical CHECK (url = trim(url))
) STRICT;

CREATE TABLE profile_profile_pdf_sources (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    pdf_extraction_id TEXT NOT NULL,
    source_type TEXT NOT NULL DEFAULT 'resume_pdf',
    created_at TEXT NOT NULL,
    CONSTRAINT profile_pdf_sources_pkey PRIMARY KEY (id),
    CONSTRAINT profile_pdf_sources_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT profile_pdf_sources_pdf_extraction_id_fk FOREIGN KEY (pdf_extraction_id) REFERENCES document_pdf_extractions(id) ON DELETE RESTRICT,
    CONSTRAINT profile_pdf_sources_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT profile_pdf_sources_source_type_not_blank CHECK (trim(source_type) <> '')
) STRICT;

CREATE TABLE profile_profile_personal_details (
    profile_id TEXT NOT NULL,
    date_of_birth TEXT,
    nationality TEXT,
    photo_document_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT profile_personal_details_pkey PRIMARY KEY (profile_id),
    CONSTRAINT profile_personal_details_photo_document_id_fk FOREIGN KEY (photo_document_id) REFERENCES document_documents(id) ON DELETE SET NULL,
    CONSTRAINT profile_personal_details_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT profile_personal_details_date_of_birth_valid CHECK (date_of_birth IS NULL OR date(date_of_birth) = date_of_birth),
    CONSTRAINT profile_personal_details_nationality_not_blank CHECK (nationality IS NULL OR trim(nationality) <> ''),
    CONSTRAINT profile_personal_details_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE profile_profile_resume_documents (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    resume_type TEXT NOT NULL DEFAULT 'master_resume',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT profile_resume_documents_pkey PRIMARY KEY (id),
    CONSTRAINT profile_resume_documents_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT profile_resume_documents_document_id_fk FOREIGN KEY (document_id) REFERENCES document_documents(id) ON DELETE RESTRICT,
    CONSTRAINT profile_resume_documents_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT profile_resume_documents_file_path_not_blank CHECK (trim(file_path) <> ''),
    CONSTRAINT profile_resume_documents_resume_type_not_blank CHECK (trim(resume_type) <> ''),
    CONSTRAINT profile_resume_documents_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE profile_profile_skills (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    skill TEXT NOT NULL,
    normalized_skill TEXT NOT NULL,
    category TEXT,
    proficiency TEXT,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    CONSTRAINT profile_skills_pkey PRIMARY KEY (id),
    CONSTRAINT profile_skills_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT profile_skills_profile_normalized_unique UNIQUE (profile_id, normalized_skill),
    CONSTRAINT profile_skills_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT profile_skills_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT profile_skills_normalized_skill_not_blank CHECK (trim(normalized_skill) <> ''),
    CONSTRAINT profile_skills_skill_not_blank CHECK (trim(skill) <> ''),
    CONSTRAINT profile_skills_category_canonical CHECK (category IS NULL OR (category = trim(category) AND category <> '')),
    CONSTRAINT profile_skills_normalized_skill_canonical CHECK (normalized_skill = lower(trim(normalized_skill))),
    CONSTRAINT profile_skills_skill_canonical CHECK (skill = trim(skill))
) STRICT;

CREATE TABLE profile_projects (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    name TEXT,
    start_date TEXT,
    end_date TEXT,
    url TEXT,
    description TEXT,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    CONSTRAINT projects_pkey PRIMARY KEY (id),
    CONSTRAINT projects_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT projects_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT projects_start_date_valid CHECK (start_date IS NULL OR date(start_date) = start_date),
    CONSTRAINT projects_end_date_valid CHECK (end_date IS NULL OR date(end_date) = end_date),
    CONSTRAINT projects_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT projects_description_canonical CHECK (description IS NULL OR (description = trim(description) AND description <> '')),
    CONSTRAINT projects_name_canonical CHECK (name IS NULL OR (name = trim(name) AND name <> '')),
    CONSTRAINT projects_url_canonical CHECK (url IS NULL OR (url = trim(url) AND url <> ''))
) STRICT;

CREATE TABLE profile_project_technologies (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    technology TEXT NOT NULL,
    normalized_technology TEXT NOT NULL,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    CONSTRAINT project_technologies_pkey PRIMARY KEY (id),
    CONSTRAINT project_technologies_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT project_technologies_project_normalized_unique UNIQUE (project_id, normalized_technology),
    CONSTRAINT project_technologies_project_id_fk FOREIGN KEY (project_id) REFERENCES profile_projects(id) ON DELETE CASCADE,
    CONSTRAINT project_technologies_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT project_technologies_normalized_technology_not_blank CHECK (trim(normalized_technology) <> ''),
    CONSTRAINT project_technologies_technology_not_blank CHECK (trim(technology) <> ''),
    CONSTRAINT project_technologies_normalized_technology_canonical CHECK (normalized_technology = lower(trim(normalized_technology))),
    CONSTRAINT project_technologies_technology_canonical CHECK (technology = trim(technology))
) STRICT;

CREATE TABLE profile_search_terms (
    profile_id TEXT NOT NULL,
    field_key TEXT NOT NULL,
    term TEXT NOT NULL,
    weight INTEGER NOT NULL,
    CONSTRAINT search_terms_pkey PRIMARY KEY (profile_id, field_key, term),
    CONSTRAINT search_terms_profile_id_fkey FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT search_terms_weight_check CHECK (weight > 0 AND weight <= 32767)
) STRICT;

CREATE TABLE resume_resumes (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    format TEXT NOT NULL,
    profile_revision TEXT NOT NULL,
    job_revision TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT resumes_pkey PRIMARY KEY (id),
    CONSTRAINT resumes_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT resumes_profile_job_format_unique UNIQUE (profile_id, job_id, format),
    CONSTRAINT resumes_job_id_fk FOREIGN KEY (job_id) REFERENCES job_jobs(id) ON DELETE CASCADE,
    CONSTRAINT resumes_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT resumes_format_known CHECK (format = 'germany'),
    CONSTRAINT resumes_format_not_blank CHECK (trim(format) <> ''),
    CONSTRAINT resumes_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

-- Parent key of cover_letter_cover_letters_resume_identity_fk.
CREATE UNIQUE INDEX resumes_profile_job_id_unique ON resume_resumes (profile_id, job_id, id);

CREATE TABLE resume_resume_variants (
    id TEXT NOT NULL,
    resume_id TEXT NOT NULL,
    language TEXT NOT NULL,
    document_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT resume_variants_pkey PRIMARY KEY (id),
    CONSTRAINT resume_variants_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT resume_variants_document_id_unique UNIQUE (document_id),
    CONSTRAINT resume_variants_resume_language_unique UNIQUE (resume_id, language),
    CONSTRAINT resume_variants_document_id_fk FOREIGN KEY (document_id) REFERENCES document_documents(id) ON DELETE RESTRICT,
    CONSTRAINT resume_variants_resume_id_fk FOREIGN KEY (resume_id) REFERENCES resume_resumes(id) ON DELETE CASCADE,
    CONSTRAINT resume_variants_file_path_not_blank CHECK (trim(file_path) <> ''),
    CONSTRAINT resume_variants_language_known CHECK (language IN ('en', 'de')),
    CONSTRAINT resume_variants_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE resume_resume_sections (
    id TEXT NOT NULL,
    variant_id TEXT NOT NULL,
    section_type TEXT NOT NULL,
    title TEXT NOT NULL,
    display_order INTEGER NOT NULL DEFAULT 0,
    CONSTRAINT resume_sections_pkey PRIMARY KEY (id),
    CONSTRAINT resume_sections_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT resume_sections_variant_order_unique UNIQUE (variant_id, display_order),
    CONSTRAINT resume_sections_variant_id_fk FOREIGN KEY (variant_id) REFERENCES resume_resume_variants(id) ON DELETE CASCADE,
    CONSTRAINT resume_sections_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT resume_sections_section_type_not_blank CHECK (trim(section_type) <> ''),
    CONSTRAINT resume_sections_title_not_blank CHECK (trim(title) <> '')
) STRICT;

CREATE TABLE resume_resume_entries (
    id TEXT NOT NULL,
    section_id TEXT NOT NULL,
    entry_type TEXT NOT NULL,
    display_order INTEGER NOT NULL DEFAULT 0,
    title TEXT,
    organization TEXT,
    location TEXT,
    start_date TEXT,
    end_date TEXT,
    metadata TEXT,
    CONSTRAINT resume_entries_pkey PRIMARY KEY (id),
    CONSTRAINT resume_entries_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT resume_entries_section_id_fk FOREIGN KEY (section_id) REFERENCES resume_resume_sections(id) ON DELETE CASCADE,
    CONSTRAINT resume_entries_start_date_valid CHECK (start_date IS NULL OR date(start_date) = start_date),
    CONSTRAINT resume_entries_end_date_valid CHECK (end_date IS NULL OR date(end_date) = end_date),
    CONSTRAINT resume_entries_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT resume_entries_entry_type_not_blank CHECK (trim(entry_type) <> '')
) STRICT;

CREATE TABLE resume_resume_entry_bullets (
    id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    display_order INTEGER NOT NULL DEFAULT 0,
    text TEXT NOT NULL,
    CONSTRAINT resume_entry_bullets_pkey PRIMARY KEY (id),
    CONSTRAINT resume_entry_bullets_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT resume_entry_bullets_entry_id_fk FOREIGN KEY (entry_id) REFERENCES resume_resume_entries(id) ON DELETE CASCADE,
    CONSTRAINT resume_entry_bullets_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT resume_entry_bullets_text_not_blank CHECK (trim(text) <> '')
) STRICT;

CREATE TABLE cover_letter_cover_letters (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    resume_id TEXT NOT NULL,
    profile_revision TEXT NOT NULL,
    job_revision TEXT NOT NULL,
    resume_revision TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT cover_letters_pkey PRIMARY KEY (id),
    CONSTRAINT cover_letters_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT cover_letters_profile_job_resume_unique UNIQUE (profile_id, job_id, resume_id),
    CONSTRAINT cover_letters_job_id_fk FOREIGN KEY (job_id) REFERENCES job_jobs(id) ON DELETE CASCADE,
    CONSTRAINT cover_letters_profile_id_fk FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT cover_letters_resume_identity_fk FOREIGN KEY (profile_id, job_id, resume_id) REFERENCES resume_resumes(profile_id, job_id, id) ON DELETE CASCADE,
    CONSTRAINT cover_letters_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE cover_letter_cover_letter_variants (
    id TEXT NOT NULL,
    cover_letter_id TEXT NOT NULL,
    format TEXT NOT NULL,
    language TEXT NOT NULL,
    document_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    subject TEXT NOT NULL,
    salutation TEXT NOT NULL,
    closing TEXT NOT NULL,
    signature TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT cover_letter_variants_pkey PRIMARY KEY (id),
    CONSTRAINT cover_letter_variants_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT cover_letter_variants_cover_letter_format_language_unique UNIQUE (cover_letter_id, format, language),
    CONSTRAINT cover_letter_variants_document_id_unique UNIQUE (document_id),
    CONSTRAINT cover_letter_variants_cover_letter_id_fk FOREIGN KEY (cover_letter_id) REFERENCES cover_letter_cover_letters(id) ON DELETE CASCADE,
    CONSTRAINT cover_letter_variants_document_id_fk FOREIGN KEY (document_id) REFERENCES document_documents(id) ON DELETE RESTRICT,
    CONSTRAINT cover_letter_variants_closing_not_blank CHECK (trim(closing) <> ''),
    CONSTRAINT cover_letter_variants_file_path_not_blank CHECK (trim(file_path) <> ''),
    CONSTRAINT cover_letter_variants_format_known CHECK (format = 'germany'),
    CONSTRAINT cover_letter_variants_language_known CHECK (language = 'de'),
    CONSTRAINT cover_letter_variants_salutation_not_blank CHECK (trim(salutation) <> ''),
    CONSTRAINT cover_letter_variants_signature_not_blank CHECK (trim(signature) <> ''),
    CONSTRAINT cover_letter_variants_subject_not_blank CHECK (trim(subject) <> ''),
    CONSTRAINT cover_letter_variants_updated_at_not_before_created_at CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE cover_letter_cover_letter_paragraphs (
    id TEXT NOT NULL,
    variant_id TEXT NOT NULL,
    display_order INTEGER NOT NULL,
    text TEXT NOT NULL,
    CONSTRAINT cover_letter_paragraphs_pkey PRIMARY KEY (id),
    CONSTRAINT cover_letter_paragraphs_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT cover_letter_paragraphs_variant_order_unique UNIQUE (variant_id, display_order),
    CONSTRAINT cover_letter_paragraphs_variant_id_fk FOREIGN KEY (variant_id) REFERENCES cover_letter_cover_letter_variants(id) ON DELETE CASCADE,
    CONSTRAINT cover_letter_paragraphs_display_order_non_negative CHECK (display_order >= 0),
    CONSTRAINT cover_letter_paragraphs_text_not_blank CHECK (trim(text) <> '')
) STRICT;

CREATE TABLE match_reports (
    id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    profile_revision TEXT NOT NULL,
    job_revision TEXT NOT NULL,
    algorithm_version TEXT NOT NULL,
    overall_score INTEGER NOT NULL,
    confidence INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    blocker_mismatch INTEGER NOT NULL,
    components TEXT NOT NULL,
    evidence TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CONSTRAINT reports_pkey PRIMARY KEY (id),
    CONSTRAINT reports_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT reports_revision_unique UNIQUE (algorithm_version, profile_id, profile_revision, job_id, job_revision),
    CONSTRAINT reports_job_id_fkey FOREIGN KEY (job_id) REFERENCES job_jobs(id) ON DELETE CASCADE,
    CONSTRAINT reports_profile_id_fkey FOREIGN KEY (profile_id) REFERENCES profile_profiles(id) ON DELETE CASCADE,
    CONSTRAINT reports_algorithm_version_check CHECK (trim(algorithm_version) <> ''),
    CONSTRAINT reports_blocker_mismatch_bool CHECK (blocker_mismatch IN (0, 1)),
    CONSTRAINT reports_components_check CHECK (CASE WHEN json_valid(components) THEN json_type(components) = 'array' ELSE 0 END),
    CONSTRAINT reports_confidence_check CHECK (confidence >= 0 AND confidence <= 100),
    CONSTRAINT reports_evidence_check CHECK (CASE WHEN json_valid(evidence) THEN json_type(evidence) = 'array' ELSE 0 END),
    CONSTRAINT reports_outcome_check CHECK (outcome IN ('STRONG_MATCH', 'PARTIAL_MATCH', 'WEAK_MATCH', 'INSUFFICIENT_EVIDENCE')),
    CONSTRAINT reports_overall_score_check CHECK (overall_score >= 0 AND overall_score <= 100)
) STRICT;

CREATE TABLE match_reviews (
    id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    report_id TEXT NOT NULL,
    reviewer TEXT NOT NULL,
    model TEXT NOT NULL,
    review_version TEXT NOT NULL,
    overall_score INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    blocker_mismatch INTEGER NOT NULL,
    components TEXT NOT NULL,
    evidence TEXT NOT NULL,
    summary TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CONSTRAINT reviews_pkey PRIMARY KEY (id),
    CONSTRAINT reviews_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT reviews_fingerprint_shape CHECK (length(fingerprint) = 36 AND fingerprint NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT reviews_fingerprint_key UNIQUE (fingerprint),
    CONSTRAINT reviews_id_report_unique UNIQUE (id, report_id),
    CONSTRAINT reviews_report_id_fkey FOREIGN KEY (report_id) REFERENCES match_reports(id) ON DELETE CASCADE,
    CONSTRAINT reviews_blocker_mismatch_bool CHECK (blocker_mismatch IN (0, 1)),
    CONSTRAINT reviews_components_check CHECK (CASE WHEN json_valid(components) THEN json_type(components) = 'array' ELSE 0 END),
    CONSTRAINT reviews_evidence_check CHECK (CASE WHEN json_valid(evidence) THEN json_type(evidence) = 'array' ELSE 0 END),
    CONSTRAINT reviews_model_check CHECK (trim(model) <> ''),
    CONSTRAINT reviews_outcome_check CHECK (outcome IN ('STRONG_MATCH', 'PARTIAL_MATCH', 'WEAK_MATCH', 'INSUFFICIENT_EVIDENCE')),
    CONSTRAINT reviews_overall_score_check CHECK (overall_score >= 0 AND overall_score <= 100),
    CONSTRAINT reviews_review_version_check CHECK (trim(review_version) <> ''),
    CONSTRAINT reviews_reviewer_check CHECK (trim(reviewer) <> ''),
    CONSTRAINT reviews_summary_check CHECK (summary IN ('review_consistent', 'score_adjustment', 'outcome_adjustment', 'evidence_defect_identified'))
) STRICT;

CREATE TABLE match_disagreements (
    id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    report_id TEXT NOT NULL,
    review_id TEXT NOT NULL,
    policy_version TEXT NOT NULL,
    reasons TEXT NOT NULL,
    evidence_defect_codes TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL,
    linear_issue_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT disagreements_pkey PRIMARY KEY (id),
    CONSTRAINT disagreements_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT disagreements_fingerprint_shape CHECK (length(fingerprint) = 36 AND fingerprint NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT disagreements_fingerprint_key UNIQUE (fingerprint),
    CONSTRAINT disagreements_review_policy_unique UNIQUE (review_id, policy_version),
    CONSTRAINT disagreements_report_id_fkey FOREIGN KEY (report_id) REFERENCES match_reports(id) ON DELETE CASCADE,
    CONSTRAINT disagreements_review_id_fkey FOREIGN KEY (review_id) REFERENCES match_reviews(id) ON DELETE CASCADE,
    CONSTRAINT disagreements_review_report_fk FOREIGN KEY (review_id, report_id) REFERENCES match_reviews(id, report_id) ON DELETE CASCADE,
    -- The element-containment half of the PostgreSQL check needs a sub-query: see RULES_TO_PORT.md.
    CONSTRAINT disagreements_evidence_defect_codes_check CHECK (CASE WHEN json_valid(evidence_defect_codes) THEN json_type(evidence_defect_codes) = 'array' ELSE 0 END),
    CONSTRAINT disagreements_linear_issue_id_check CHECK (linear_issue_id IS NULL OR trim(linear_issue_id) <> ''),
    CONSTRAINT disagreements_policy_version_check CHECK (policy_version = 'divergence-v1'),
    CONSTRAINT disagreements_reasons_check CHECK (CASE WHEN json_valid(reasons) THEN json_type(reasons) = 'array' ELSE 0 END),
    CONSTRAINT disagreements_status_check CHECK (status IN ('OPEN', 'PENDING_ESCALATION', 'ACKNOWLEDGED', 'LINKED'))
) STRICT;

CREATE TABLE jobniac_generated_asset_roots (
    id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CONSTRAINT generated_asset_roots_pkey PRIMARY KEY (id),
    CONSTRAINT generated_asset_roots_id_shape CHECK (length(id) = 36 AND id NOT GLOB '*[^0-9a-f-]*')
) STRICT;

CREATE TABLE jobniac_generated_assets (
    asset_id TEXT NOT NULL,
    root_id TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    document_id TEXT,
    lifecycle TEXT NOT NULL,
    created_at TEXT NOT NULL,
    materialized_at TEXT,
    retired_at TEXT,
    deleted_at TEXT,
    retirement_id TEXT,
    generation_intent_required INTEGER NOT NULL DEFAULT 0,
    -- Bumped by every UPDATE (trigger below). Replaces the PostgreSQL xmin read of the
    -- "created in this transaction" admission checks.
    row_version INTEGER NOT NULL DEFAULT 0,
    CONSTRAINT generated_assets_pkey PRIMARY KEY (asset_id),
    CONSTRAINT generated_assets_asset_id_shape CHECK (length(asset_id) = 36 AND asset_id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT generated_assets_retirement_id_shape CHECK (retirement_id IS NULL OR (length(retirement_id) = 36 AND retirement_id NOT GLOB '*[^0-9a-f-]*')),
    CONSTRAINT generated_assets_root_id_fk FOREIGN KEY (root_id) REFERENCES jobniac_generated_asset_roots(id) ON DELETE RESTRICT,
    CONSTRAINT generated_assets_document_id_fk FOREIGN KEY (document_id) REFERENCES document_documents(id) ON DELETE RESTRICT,
    CONSTRAINT generated_assets_document_id_key UNIQUE (document_id),
    CONSTRAINT generated_assets_retirement_id_key UNIQUE (retirement_id),
    CONSTRAINT generated_assets_root_path_unique UNIQUE (root_id, relative_path),
    CONSTRAINT generated_assets_asset_retirement_unique UNIQUE (asset_id, retirement_id),
    CONSTRAINT generated_assets_sha256_hex CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    CONSTRAINT generated_assets_size_bytes_range CHECK (size_bytes BETWEEN 1 AND 10485760),
    CONSTRAINT generated_assets_lifecycle_known CHECK (lifecycle IN ('RESERVED', 'ACTIVE', 'RETIRING', 'DELETED')),
    CONSTRAINT generated_assets_path_is_asset_pdf CHECK (relative_path = asset_id || '.pdf'),
    CONSTRAINT generated_assets_retirement_id_shape_lifecycle CHECK ((lifecycle IN ('RETIRING', 'DELETED')) = (retirement_id IS NOT NULL)),
    CONSTRAINT generated_assets_retired_at_shape_lifecycle CHECK ((lifecycle IN ('RETIRING', 'DELETED')) = (retired_at IS NOT NULL)),
    CONSTRAINT generated_assets_deleted_at_shape_lifecycle CHECK ((lifecycle = 'DELETED') = (deleted_at IS NOT NULL)),
    CONSTRAINT generated_assets_active_materialized CHECK (lifecycle <> 'ACTIVE' OR materialized_at IS NOT NULL),
    CONSTRAINT generated_assets_intent_required_bool CHECK (generation_intent_required IN (0, 1)),
    CONSTRAINT generated_assets_row_version_non_negative CHECK (row_version >= 0)
) STRICT;

CREATE TABLE jobniac_generated_asset_cleanups (
    task_id TEXT NOT NULL,
    asset_id TEXT NOT NULL,
    retirement_id TEXT NOT NULL,
    status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    lease_token TEXT,
    lease_expires_at TEXT,
    failure_category TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    CONSTRAINT generated_asset_cleanups_pkey PRIMARY KEY (task_id),
    CONSTRAINT generated_asset_cleanups_task_id_shape CHECK (length(task_id) = 36 AND task_id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT generated_asset_cleanups_lease_token_shape CHECK (lease_token IS NULL OR (length(lease_token) = 36 AND lease_token NOT GLOB '*[^0-9a-f-]*')),
    CONSTRAINT generated_asset_cleanups_asset_id_key UNIQUE (asset_id),
    CONSTRAINT generated_asset_cleanups_asset_retirement_fk FOREIGN KEY (asset_id, retirement_id) REFERENCES jobniac_generated_assets(asset_id, retirement_id) ON DELETE RESTRICT,
    CONSTRAINT generated_asset_cleanups_status_known CHECK (status IN ('PENDING', 'PROCESSING', 'COMPLETED', 'BLOCKED')),
    CONSTRAINT generated_asset_cleanups_attempt_count_check CHECK (attempt_count >= 0),
    CONSTRAINT generated_asset_cleanups_failure_category_known CHECK (failure_category IN ('FILE_DELETE_FAILED', 'IDENTITY_MISMATCH', 'OWNERSHIP_UNPROVEN', 'UNSUPPORTED_PLATFORM')),
    CONSTRAINT generated_asset_cleanups_lease_token_shape_status CHECK ((status = 'PROCESSING') = (lease_token IS NOT NULL)),
    CONSTRAINT generated_asset_cleanups_lease_expires_shape_status CHECK ((status = 'PROCESSING') = (lease_expires_at IS NOT NULL)),
    CONSTRAINT generated_asset_cleanups_completed_at_shape_status CHECK ((status = 'COMPLETED') = (completed_at IS NOT NULL))
) STRICT;

CREATE TABLE jobniac_generation_intents (
    asset_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    purpose TEXT NOT NULL,
    profile_id TEXT,
    job_id TEXT,
    resume_id TEXT,
    output_scope TEXT,
    state TEXT NOT NULL,
    lease_token TEXT,
    lease_started_at TEXT,
    lease_expires_at TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT,
    CONSTRAINT generation_intents_pkey PRIMARY KEY (asset_id),
    CONSTRAINT generation_intents_operation_id_shape CHECK (length(operation_id) = 36 AND operation_id NOT GLOB '*[^0-9a-f-]*'),
    CONSTRAINT generation_intents_lease_token_shape CHECK (lease_token IS NULL OR (length(lease_token) = 36 AND lease_token NOT GLOB '*[^0-9a-f-]*')),
    CONSTRAINT generation_intents_asset_id_fk FOREIGN KEY (asset_id) REFERENCES jobniac_generated_assets(asset_id) ON DELETE RESTRICT,
    CONSTRAINT generation_intents_purpose_known CHECK (purpose IN ('STANDALONE_PDF', 'PROFILE_RESUME', 'GERMAN_RESUME', 'GERMAN_COVER_LETTER')),
    CONSTRAINT generation_intents_state_known CHECK (state IN ('PENDING', 'COMPLETED', 'ABANDONED')),
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
    -- The PostgreSQL check also bounds the lease to five minutes (interval arithmetic): see
    -- RULES_TO_PORT.md.
    CONSTRAINT generation_intents_state_shape CHECK (
        (state = 'PENDING' AND lease_token IS NOT NULL AND lease_started_at IS NOT NULL
            AND lease_expires_at IS NOT NULL AND finished_at IS NULL
            AND lease_started_at >= created_at AND lease_expires_at > lease_started_at)
        OR (state IN ('COMPLETED', 'ABANDONED') AND lease_token IS NULL
            AND lease_started_at IS NULL AND lease_expires_at IS NULL
            AND finished_at IS NOT NULL AND finished_at >= created_at)
    )
) STRICT;

-- ---------------------------------------------------------------------------------------------
-- Indexes
-- ---------------------------------------------------------------------------------------------

CREATE INDEX idx_cover_letter_paragraphs_variant_order ON cover_letter_cover_letter_paragraphs (variant_id, display_order);
CREATE INDEX idx_cover_letter_variants_cover_letter_id ON cover_letter_cover_letter_variants (cover_letter_id);
CREATE INDEX idx_cover_letters_job_id ON cover_letter_cover_letters (job_id);
CREATE INDEX idx_cover_letters_profile_id ON cover_letter_cover_letters (profile_id);
CREATE INDEX idx_cover_letters_resume_id ON cover_letter_cover_letters (resume_id);

CREATE INDEX idx_document_blobs_created_at ON document_blobs (created_at DESC, id);
CREATE UNIQUE INDEX idx_document_blobs_sha256_unique ON document_blobs (sha256);
CREATE INDEX idx_document_documents_blob_id ON document_documents (blob_id);
CREATE INDEX idx_document_documents_created_at ON document_documents (created_at DESC, id);
CREATE INDEX idx_document_documents_media_type ON document_documents (media_type);
CREATE INDEX idx_generated_resume_file_cleanups_completed_retention ON document_generated_resume_file_cleanups (completed_at, id) WHERE status = 'COMPLETED';
CREATE INDEX idx_generated_resume_file_cleanups_due ON document_generated_resume_file_cleanups (next_attempt_at, created_at) WHERE status IN ('PENDING', 'PROCESSING');
CREATE INDEX idx_pdf_extractions_file_created_at ON document_pdf_extractions (file_id, created_at DESC, id);
CREATE UNIQUE INDEX idx_pdf_extractions_file_id_unique ON document_pdf_extractions (file_id);

CREATE INDEX idx_job_link_ingestions_job_id ON job_job_link_ingestions (job_id);
CREATE INDEX idx_job_skills_job_order ON job_job_skills (job_id, display_order);
CREATE INDEX idx_job_skills_normalized ON job_job_skills (normalized_skill);
CREATE INDEX idx_job_text_ingestions_job_id ON job_job_text_ingestions (job_id);
CREATE INDEX idx_jobs_company ON job_jobs (company);
CREATE INDEX idx_jobs_posted_at ON job_jobs (posted_at DESC);
CREATE INDEX idx_jobs_source_method ON job_jobs (source_method);
CREATE INDEX idx_jobs_title ON job_jobs (title);
-- `weight` is the trailing column where PostgreSQL used INCLUDE (weight).
CREATE INDEX job_search_terms_term_prefix_idx ON job_search_terms (term, job_id, field_key, weight);
CREATE INDEX jobs_list_created_id_idx ON job_jobs (created_at DESC, id);

CREATE INDEX disagreements_list_created_id_idx ON match_disagreements (created_at DESC, id DESC);
CREATE INDEX disagreements_report_created_id_idx ON match_disagreements (report_id, created_at DESC, id DESC);
CREATE INDEX reports_job_created_id_idx ON match_reports (job_id, created_at DESC, id DESC);
CREATE INDEX reports_list_created_id_idx ON match_reports (created_at DESC, id DESC);
CREATE INDEX reports_profile_created_id_idx ON match_reports (profile_id, created_at DESC, id DESC);
-- PostgreSQL indexed the array ARRAY[profile_id, job_id]; the row-value pair is the equivalent key.
CREATE INDEX reports_profile_job_created_id_idx ON match_reports (profile_id, job_id, created_at DESC, id DESC);
CREATE INDEX reviews_list_created_id_idx ON match_reviews (created_at DESC, id DESC);
CREATE INDEX reviews_report_created_id_idx ON match_reviews (report_id, created_at DESC, id DESC);

CREATE UNIQUE INDEX idx_education_canonical_unique ON profile_education (profile_id, COALESCE(lower(trim(institution)), ''), COALESCE(lower(trim(degree)), ''), COALESCE(lower(trim(field)), ''), COALESCE(start_date, '0001-01-01'), COALESCE(end_date, '9999-12-31'));
CREATE INDEX idx_education_profile_id ON profile_education (profile_id);
CREATE UNIQUE INDEX idx_experiences_canonical_unique ON profile_experiences (profile_id, COALESCE(lower(trim(company)), ''), COALESCE(lower(trim(title)), ''), COALESCE(start_date, '0001-01-01'), COALESCE(end_date, '9999-12-31'));
CREATE INDEX idx_experiences_company ON profile_experiences (company);
CREATE INDEX idx_experiences_profile_order ON profile_experiences (profile_id, display_order);
CREATE INDEX idx_profile_contacts_profile_id ON profile_profile_contacts (profile_id);
CREATE INDEX idx_profile_contacts_type ON profile_profile_contacts (contact_type);
CREATE UNIQUE INDEX idx_profile_contacts_type_value_canonical_unique ON profile_profile_contacts (profile_id, lower(trim(contact_type)), trim(contact_value));
CREATE INDEX idx_profile_languages_normalized ON profile_profile_languages (normalized_language);
CREATE UNIQUE INDEX idx_profile_languages_normalized_canonical_unique ON profile_profile_languages (profile_id, lower(trim(normalized_language)));
CREATE INDEX idx_profile_languages_profile_order ON profile_profile_languages (profile_id, display_order);
CREATE INDEX idx_profile_links_profile_id ON profile_profile_links (profile_id);
CREATE INDEX idx_profile_links_type ON profile_profile_links (link_type);
CREATE UNIQUE INDEX idx_profile_links_type_url_canonical_unique ON profile_profile_links (profile_id, lower(trim(link_type)), trim(url));
CREATE INDEX idx_profile_pdf_sources_created_at ON profile_profile_pdf_sources (created_at DESC, id);
CREATE UNIQUE INDEX idx_profile_pdf_sources_pdf_extraction_id_unique ON profile_profile_pdf_sources (pdf_extraction_id);
CREATE UNIQUE INDEX idx_profile_pdf_sources_profile_id_unique ON profile_profile_pdf_sources (profile_id);
CREATE INDEX idx_profile_personal_details_photo_document_id ON profile_profile_personal_details (photo_document_id) WHERE photo_document_id IS NOT NULL;
CREATE UNIQUE INDEX idx_profile_resume_documents_document_id_unique ON profile_profile_resume_documents (document_id);
CREATE UNIQUE INDEX idx_profile_resume_documents_file_path_unique ON profile_profile_resume_documents (file_path);
CREATE UNIQUE INDEX idx_profile_resume_documents_profile_id_resume_type_unique ON profile_profile_resume_documents (profile_id, resume_type);
CREATE INDEX idx_profile_resume_documents_updated_at ON profile_profile_resume_documents (updated_at DESC, id);
CREATE INDEX idx_profile_skills_normalized ON profile_profile_skills (normalized_skill);
CREATE UNIQUE INDEX idx_profile_skills_normalized_canonical_unique ON profile_profile_skills (profile_id, lower(trim(normalized_skill)));
CREATE INDEX idx_profile_skills_profile_order ON profile_profile_skills (profile_id, display_order);
CREATE UNIQUE INDEX idx_profiles_email_canonical_unique ON profile_profiles (lower(trim(email)));
CREATE INDEX idx_profiles_full_name ON profile_profiles (full_name);
CREATE INDEX idx_project_technologies_normalized ON profile_project_technologies (normalized_technology);
CREATE UNIQUE INDEX idx_project_technologies_normalized_canonical_unique ON profile_project_technologies (project_id, lower(trim(normalized_technology)));
CREATE INDEX idx_project_technologies_project_order ON profile_project_technologies (project_id, display_order);
CREATE UNIQUE INDEX idx_projects_canonical_unique ON profile_projects (profile_id, COALESCE(lower(trim(name)), ''), COALESCE(trim(url), ''));
CREATE INDEX idx_projects_profile_order ON profile_projects (profile_id, display_order);
CREATE INDEX profile_search_terms_term_prefix_idx ON profile_search_terms (term, profile_id, field_key, weight);
CREATE INDEX profiles_list_created_id_idx ON profile_profiles (created_at DESC, id);

CREATE INDEX idx_resume_entries_section_order ON resume_resume_entries (section_id, display_order);
CREATE INDEX idx_resume_entry_bullets_entry_order ON resume_resume_entry_bullets (entry_id, display_order);
CREATE INDEX idx_resume_sections_variant_order ON resume_resume_sections (variant_id, display_order);
CREATE INDEX idx_resume_variants_resume_id ON resume_resume_variants (resume_id);
CREATE INDEX idx_resumes_job_id ON resume_resumes (job_id);
CREATE INDEX idx_resumes_profile_id ON resume_resumes (profile_id);
CREATE INDEX idx_resumes_updated_at ON resume_resumes (updated_at DESC, id);

CREATE INDEX generated_asset_cleanups_due_idx ON jobniac_generated_asset_cleanups (next_attempt_at, task_id) WHERE status IN ('PENDING', 'PROCESSING');
CREATE INDEX generated_asset_cleanups_completed_idx ON jobniac_generated_asset_cleanups (completed_at, task_id) WHERE status = 'COMPLETED';
-- PostgreSQL NULLS NOT DISTINCT: a NULL output scope (stand-alone PDF) takes part in uniqueness.
-- The scope CHECK allows NULL only for STANDALONE_PDF, and never the empty string, so the
-- COALESCE sentinel cannot collide with a real scope.
CREATE UNIQUE INDEX generation_intents_operation_output_unique ON jobniac_generation_intents (operation_id, purpose, COALESCE(output_scope, ''));
CREATE INDEX generation_intents_pending_expiry_idx ON jobniac_generation_intents (lease_expires_at, asset_id) WHERE state = 'PENDING';

-- ---------------------------------------------------------------------------------------------
-- Row-local immutability triggers (no sub-query, no lock): PostgreSQL V0001, V0005 and V0006.
-- Everything else those PL/pgSQL guards did is in RULES_TO_PORT.md.
-- ---------------------------------------------------------------------------------------------

CREATE TRIGGER reports_immutable BEFORE UPDATE ON match_reports
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'match history is immutable');
END;

CREATE TRIGGER reviews_immutable BEFORE UPDATE ON match_reviews
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'match history is immutable');
END;

CREATE TRIGGER generated_asset_birth_state BEFORE INSERT ON jobniac_generated_assets
FOR EACH ROW WHEN NEW.lifecycle <> 'RESERVED' OR NEW.retirement_id IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'Owned assets begin with durable reservation');
END;

CREATE TRIGGER generated_asset_no_delete BEFORE DELETE ON jobniac_generated_assets
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'Owned asset tombstones must be retained');
END;

CREATE TRIGGER generated_asset_identity_immutable BEFORE UPDATE ON jobniac_generated_assets
FOR EACH ROW WHEN NEW.asset_id IS NOT OLD.asset_id OR NEW.root_id IS NOT OLD.root_id
    OR NEW.relative_path IS NOT OLD.relative_path OR NEW.sha256 IS NOT OLD.sha256
    OR NEW.size_bytes IS NOT OLD.size_bytes OR NEW.document_id IS NOT OLD.document_id
    OR NEW.created_at IS NOT OLD.created_at BEGIN
    SELECT RAISE(ABORT, 'Owned asset identity is immutable');
END;

CREATE TRIGGER generated_asset_retirement_permanent BEFORE UPDATE ON jobniac_generated_assets
FOR EACH ROW WHEN OLD.retirement_id IS NOT NULL
    AND (NEW.retirement_id IS NOT OLD.retirement_id OR NEW.retired_at IS NOT OLD.retired_at) BEGIN
    SELECT RAISE(ABORT, 'Retirement identity is permanent');
END;

CREATE TRIGGER generated_asset_lifecycle_forward_only BEFORE UPDATE ON jobniac_generated_assets
FOR EACH ROW WHEN NOT (
    NEW.lifecycle = OLD.lifecycle
    OR (OLD.lifecycle = 'RESERVED' AND NEW.lifecycle IN ('ACTIVE', 'RETIRING'))
    OR (OLD.lifecycle = 'ACTIVE' AND NEW.lifecycle = 'RETIRING')
    OR (OLD.lifecycle = 'RETIRING' AND NEW.lifecycle = 'DELETED')
) BEGIN
    SELECT RAISE(ABORT, 'Owned asset lifecycle cannot regress');
END;

CREATE TRIGGER generation_birth_marker_immutable BEFORE UPDATE ON jobniac_generated_assets
FOR EACH ROW WHEN NEW.generation_intent_required IS NOT OLD.generation_intent_required BEGIN
    SELECT RAISE(ABORT, 'Generation birth classification is immutable');
END;

-- The WHEN guard stops the bump from re-entering itself.
CREATE TRIGGER generated_asset_row_version AFTER UPDATE ON jobniac_generated_assets
FOR EACH ROW WHEN NEW.row_version = OLD.row_version BEGIN
    UPDATE jobniac_generated_assets SET row_version = OLD.row_version + 1
    WHERE asset_id = NEW.asset_id;
END;

CREATE TRIGGER generated_cleanup_identity_immutable BEFORE UPDATE ON jobniac_generated_asset_cleanups
FOR EACH ROW WHEN NEW.task_id IS NOT OLD.task_id OR NEW.asset_id IS NOT OLD.asset_id
    OR NEW.retirement_id IS NOT OLD.retirement_id OR NEW.created_at IS NOT OLD.created_at BEGIN
    SELECT RAISE(ABORT, 'Cleanup intent identity is immutable');
END;

CREATE TRIGGER generated_cleanup_completed_immutable BEFORE UPDATE ON jobniac_generated_asset_cleanups
FOR EACH ROW WHEN OLD.status = 'COMPLETED' AND (
    NEW.status IS NOT OLD.status OR NEW.attempt_count IS NOT OLD.attempt_count
    OR NEW.next_attempt_at IS NOT OLD.next_attempt_at OR NEW.lease_token IS NOT OLD.lease_token
    OR NEW.lease_expires_at IS NOT OLD.lease_expires_at
    OR NEW.failure_category IS NOT OLD.failure_category
    OR NEW.updated_at IS NOT OLD.updated_at OR NEW.completed_at IS NOT OLD.completed_at
) BEGIN
    SELECT RAISE(ABORT, 'Completed cleanup intent is immutable');
END;

CREATE TRIGGER generation_intent_no_delete BEFORE DELETE ON jobniac_generation_intents
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'Generation intent history must be retained');
END;

CREATE TRIGGER generation_intent_identity_immutable BEFORE UPDATE ON jobniac_generation_intents
FOR EACH ROW WHEN NEW.asset_id IS NOT OLD.asset_id OR NEW.operation_id IS NOT OLD.operation_id
    OR NEW.purpose IS NOT OLD.purpose OR NEW.profile_id IS NOT OLD.profile_id
    OR NEW.job_id IS NOT OLD.job_id OR NEW.resume_id IS NOT OLD.resume_id
    OR NEW.output_scope IS NOT OLD.output_scope OR NEW.created_at IS NOT OLD.created_at BEGIN
    SELECT RAISE(ABORT, 'Generation intent identity is immutable');
END;

CREATE TRIGGER generation_intent_terminal_immutable BEFORE UPDATE ON jobniac_generation_intents
FOR EACH ROW WHEN OLD.state <> 'PENDING' AND (
    NEW.state IS NOT OLD.state OR NEW.lease_token IS NOT OLD.lease_token
    OR NEW.lease_started_at IS NOT OLD.lease_started_at
    OR NEW.lease_expires_at IS NOT OLD.lease_expires_at OR NEW.finished_at IS NOT OLD.finished_at
) BEGIN
    SELECT RAISE(ABORT, 'Terminal generation intent is immutable');
END;
