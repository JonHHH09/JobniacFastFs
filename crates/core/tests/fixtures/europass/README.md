# Europass fixtures

Both files are the `attachment.xml` embedded in PDFs exported from the official Europass editor.
All content is fictional (person "Alex Sample", invented employers, `example.com` addresses and
a US test phone number); no real personal data.

- `cv-alex-sample.xml`: HR-XML based `Candidate` CV (schema version 4.0, namespace
  `http://www.europass.eu/1.0`).
- `cover-letter-alex-sample.xml`: legacy Europass v3.4 `SkillsPassport` cover letter
  (`DocumentType` `ECL`).

They are kept byte for byte as exported. The expected parse results are the golden files
`../golden/europass-cv-alex-sample.json` and `../golden/europass-cover-letter-alex-sample.json`.
