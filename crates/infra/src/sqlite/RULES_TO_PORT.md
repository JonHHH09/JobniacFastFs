# Rules to port into the SQLite repositories (temporary)

This file is deleted when the repository port is complete. It lists every PostgreSQL rule that
`sql/sqlite/V0001__baseline.sql` does not enforce, so the repository porters re-implement it in
Rust. Source lines refer to `crates/infra/sql/V000*.sql` as of the branch point. A rule that
needs the whole transaction state (the deferred ones) runs as a pre-commit check registered with
`SqliteTransaction::before_commit`. A failing check returns `SqliteError::constraint(..)`, which
maps to the same `AppError::PersistenceConstraint` class (23514) as the PostgreSQL trigger.

Advisory locks (`pg_advisory_xact_lock`, `pg_try_advisory_xact_lock`, `hashtextextended`) and
`FOR SHARE` reads in the V0005/V0006 guards are dropped without a replacement: every write
transaction is `BEGIN IMMEDIATE`, so writers are already serialized.

## Rules not in SQL (27)

### V0001

1. `job_link_ingestions_normalized_url_safe_identity` (V0001:197): `normalized_url` must not start
   with `http://` or `https://` followed by userinfo (`^https?://[^/?#]*@`, case-insensitive). The
   `#` half of the check is in SQL.
2. `job_link_ingestions_url_safe_display` (V0001:200): same userinfo rule for `url`. The `?` and
   `#` halves are in SQL.
3. `disagreements_evidence_defect_codes_check` (V0001:269): every element of
   `evidence_defect_codes` is one of `structured_evidence_missing`, `structured_evidence_incorrect`,
   `requirement_provenance_missing`, `outcome_calibration_issue`. The array type check is in SQL.
4. Job source provenance (V0001:22-77 functions, V0001:948, 950, 952 deferred constraint
   triggers on `job_job_link_ingestions`, `job_job_text_ingestions`, `job_jobs`): at commit, every
   touched job (new or old `job_id`/`id`) with `source_method = 'link'` has exactly one link
   ingestion row and no text row, and `'text'` has exactly one text row and no link row. Pre-commit
   check.
5. Canonical lowercase (V0001:258-331 `lower(btrim(..))` checks and the canonical unique indexes):
   PostgreSQL `lower()` is Unicode-aware, SQLite `lower()` is ASCII-only. The SQL checks and
   indexes still exist but only cover ASCII. Rust must normalize with Unicode lowercase before
   every write (emails, contact and link types, normalized skills, languages and technologies)
   and treat Rust as the source of truth for what counts as a duplicate.

### V0006 (check constraint)

6. `generation_intents_state_shape` (V0006:42-56): `lease_expires_at <= lease_started_at + 5 minutes`
   for a PENDING intent. The other terms of the check are in SQL.

### V0005 (owned generated assets and cleanup)

7. Adoption (V0005:109-113, `generated_asset_referenced` V0005:82-89, `generated_document_referenced`
   V0005:64-80): inserting an asset row (RESERVED) is refused when its document or its
   `relative_path` is already referenced by a profile resume document, a resume variant, a cover
   letter variant, a personal-details photo, a PDF source, or (V0006 definition, rule 27) a completed
   stand-alone intent.
8. Document identity (V0005:114-122): on insert with a `document_id`, the document's blob must
   match the asset's `sha256` and `size_bytes`.
9. Referenced assets cannot retire (V0005:144-148): moving to RETIRING or DELETED is refused while
   `generated_asset_referenced` holds.
10. Deletion needs a completed intent (V0005:149-156): an asset becomes DELETED only when a
    COMPLETED cleanup task exists for the same `asset_id` and `retirement_id`.
11. Cleanup retention (V0005:174-182): a cleanup task may be deleted only when it is COMPLETED and
    its asset is DELETED with the same `retirement_id`.
12. Cleanup needs a retired identity (V0005:191-198): inserting or updating a cleanup task requires
    an asset with the same `retirement_id` in RETIRING or DELETED.
13. Attachable documents (`assert_generated_document_attachable`, V0005:205-225): a document that
    maps to an owned asset can be attached only while that asset is ACTIVE. Applied by rules 14
    and 15.
14. Reference guards (V0005:227-295, five triggers on `profile_profile_resume_documents`,
    `resume_resume_variants`, `cover_letter_cover_letter_variants` for `document_id`,
    `profile_profile_personal_details` for `photo_document_id`, `document_pdf_extractions` for
    `file_id`): on insert or update, rule 13 holds for the document. For the three resume and
    cover-letter tables, additionally: a `file_path` that maps to an owned asset must belong to
    the same document (`Generated paths cannot be rebound`), and a document that has an owned asset
    must keep that asset's `relative_path` (`Generated document path is immutable`).
15. PDF source guard (V0005:297-313, on `profile_profile_pdf_sources.pdf_extraction_id`): rule 13
    holds for the extraction's document.
16. Owned document content (V0005:315-332, on `document_documents.blob_id`): `blob_id` of a document
    that has an owned asset cannot change.
17. Owned blob content (V0005:334-361, on `document_blobs`): `sha256`, `byte_size` and `content` of
    a blob used by a document that has an owned asset cannot change.

### V0006 (generation intents)

18. Birth intent (V0006:67-81, deferred): at commit, an asset inserted with
    `generation_intent_required = 1` has a row in `jobniac_generation_intents`. Pre-commit check.
19. Intent needs a document (V0006:98-101): the intent's asset has a `document_id`.
20. Intent birth (V0006:109-123, uses `xmin = current transaction`): an intent is inserted only for
    an asset that is RESERVED, has `generation_intent_required = 1`, was inserted by the same
    transaction (use `jobniac_generated_assets.row_version` or track it in the transaction), the
    intent is PENDING, `created_at` equals the asset's `created_at` and `lease_started_at` equals
    `created_at`.
21. Lease renewal (V0006:134-143): a PENDING intent updated to PENDING must rotate `lease_token`,
    keep `lease_started_at >= old lease_started_at` and `< old lease_expires_at`, and keep
    `lease_expires_at >= old lease_expires_at`.
22. Finish window (V0006:144-149): a terminal state needs `finished_at >= old lease_started_at`,
    and COMPLETED also needs `finished_at < old lease_expires_at`.
23. Pending needs an asset (V0006:151-155): a PENDING intent requires its asset to be RESERVED or
    ACTIVE.
24. Completion proof (V0006:156-193): completing an intent requires an ACTIVE asset and, by
    `purpose`, the exact link: STANDALONE_PDF none; PROFILE_RESUME a `profile_profile_resume_documents`
    row for (profile, `resume_type = output_scope`, document, path); GERMAN_RESUME a
    `resume_resume_variants` row under a `germany` resume of the same profile, job and resume with
    `language = output_scope`; GERMAN_COVER_LETTER the equivalent `cover_letter_cover_letter_variants`
    row.
25. Pending retirement (V0006:202-219): an asset cannot move to RETIRING or DELETED while a PENDING
    intent exists for it.
26. Abandonment retirement (V0006:221-243, deferred): at commit, an ABANDONED intent's asset is
    RETIRING or DELETED. Pre-commit check.
27. Referenced documents (V0006:245-268): `generated_document_referenced` also counts a COMPLETED
    STANDALONE_PDF intent whose asset has the document. Feeds rules 7 and 9.

## In SQL (for reference)

Row-local `RAISE(ABORT)` triggers: match report and review immutability; asset birth state,
no delete, identity, retirement identity, forward-only lifecycle, birth marker; cleanup identity
and completed immutability; intent no delete, identity and terminal immutability. The asset
`row_version` bump trigger replaces `xmin`.

## Conventions the Rust writers must follow

* Timestamps: one fixed-width RFC 3339 UTC format for every column, because comparisons in CHECK
  constraints and indexes are string comparisons.
* JSON columns: write canonical JSON text; SQLite does not normalize it like `jsonb`.
* Tables of PostgreSQL V0002 (`jobniac.import_runs`, `jobniac.import_cleanup_decisions`) are not
  created: there is no PostgreSQL import.
