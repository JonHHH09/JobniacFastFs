-- Jobniac-owned cutover ledger (JN-002).
--
-- import_runs records each committed importer run: the verified source schema version, digests of
-- the source migration history and of the human-reviewed resolution map, and the reconciled
-- per-table manifest (row counts and ordered identity digests). A run writes its row in the same
-- transaction as the imported data, so a failed run leaves no ledger row and no partial data.
--
-- import_cleanup_decisions is the audit trail for imported generated-file cleanup records. Legacy
-- cleanup rows carry absolute job-engine-spring paths that must never be executed: a pending row
-- is either rehomed to a path relative to the Jobniac-owned asset root or retired as completed.
-- The legacy status and path are retained here verbatim. No foreign key points at the cleanup
-- table, so later pruning of expired cleanup rows cannot erase this audit history.

CREATE TABLE jobniac.import_runs (
    id uuid NOT NULL,
    source_schema_version integer NOT NULL,
    source_history_sha256 character(64) NOT NULL,
    resolution_map_sha256 character(64) NOT NULL,
    table_manifest jsonb NOT NULL,
    started_at timestamp with time zone NOT NULL,
    completed_at timestamp with time zone NOT NULL,
    CONSTRAINT import_runs_pkey PRIMARY KEY (id),
    CONSTRAINT import_runs_source_schema_version_positive CHECK ((source_schema_version > 0)),
    CONSTRAINT import_runs_source_history_sha256_hex CHECK ((source_history_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT import_runs_resolution_map_sha256_hex CHECK ((resolution_map_sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT import_runs_table_manifest_object CHECK ((jsonb_typeof(table_manifest) = 'object'::text)),
    CONSTRAINT import_runs_completed_not_before_started CHECK ((completed_at >= started_at))
);

CREATE TABLE jobniac.import_cleanup_decisions (
    cleanup_id uuid NOT NULL,
    import_run_id uuid NOT NULL,
    decision text NOT NULL,
    legacy_status text NOT NULL,
    legacy_file_path text NOT NULL,
    target_relative_path text,
    CONSTRAINT import_cleanup_decisions_pkey PRIMARY KEY (cleanup_id),
    CONSTRAINT import_cleanup_decisions_import_run_fk FOREIGN KEY (import_run_id) REFERENCES jobniac.import_runs(id),
    CONSTRAINT import_cleanup_decisions_decision_known CHECK ((decision = ANY (ARRAY['REHOMED'::text, 'RETIRED'::text]))),
    CONSTRAINT import_cleanup_decisions_legacy_status_known CHECK ((legacy_status = ANY (ARRAY['PENDING'::text, 'PROCESSING'::text]))),
    CONSTRAINT import_cleanup_decisions_rehome_has_path CHECK (((decision = 'REHOMED'::text) = (target_relative_path IS NOT NULL))),
    CONSTRAINT import_cleanup_decisions_relative_path_safe CHECK (((target_relative_path IS NULL) OR ((target_relative_path <> ''::text) AND (target_relative_path !~ '^(/|[A-Za-z]:)'::text) AND (target_relative_path !~ '\\'::text) AND (target_relative_path !~ '(^|/)\.\.?(/|$)'::text))))
);

CREATE INDEX import_cleanup_decisions_import_run_id_idx ON jobniac.import_cleanup_decisions USING btree (import_run_id);
