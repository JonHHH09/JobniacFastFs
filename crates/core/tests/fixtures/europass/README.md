# Europass fixtures

Both files are the `attachment.xml` embedded in the synthetic PDFs of
`crates/infra/tests/fixtures/pdf/europass/`. Those PDFs were generated with pdf-lib to mirror a
Europass export; they were not exported from the Europass editor (see that folder's README). All
content is fictional (person "Alex Sample", invented employers, `example.com` addresses and a US
test phone number); no real personal data.

- `cv-alex-sample.xml`: HR-XML based `Candidate` CV (schema version 4.0, namespace
  `http://www.europass.eu/1.0`).
- `cover-letter-alex-sample.xml`: legacy Europass v3.4 `SkillsPassport` cover letter
  (`DocumentType` `ECL`).

They are kept byte for byte as embedded. The expected parse results are the golden files
`../golden/europass-cv-alex-sample.json` and `../golden/europass-cover-letter-alex-sample.json`.
