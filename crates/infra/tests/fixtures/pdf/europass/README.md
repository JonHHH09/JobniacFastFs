# Europass fixtures

Two synthetic PDFs for the fictional person "Alex Sample" that mirror a Europass CV and cover
letter export: the Europass page layout plus an embedded `attachment.xml` in the Europass schemas.
They were generated with pdf-lib, not exported from the Europass editor: the document info names
pdf-lib as producer, and each file carries the generator's C2PA content-credentials manifest as a
second attachment. A real Europass export can differ in fonts, text order and attachments, so check
new parsing rules against one before relying on them. The files hold no real personal data and are
used only as test fixtures for PDF inspection (page size, document info and the embedded
`attachment.xml`). They are unmodified byte copies.

| File | Bytes | SHA-256 |
|---|---|---|
| `europass-cv-alex-sample.pdf` | 95675 | `f01b32c0f03ac5e4d10cb9897255f191dbc0ab00112590d771fe8109bde92645` |
| `europass-cover-letter-alex-sample.pdf` | 82960 | `9d00496f4e2a0fca017e3ded26ce19f3ac882aeee32c7c887cdac35e9b2d1a49` |

Unlike `corpus/` and `hostile/`, these files are not part of the recorded text extraction
(`tests/golden/pdf-extract.json`).
