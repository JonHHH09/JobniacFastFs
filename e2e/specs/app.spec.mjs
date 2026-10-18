// The default application session: every opt-in flag is unset, the database is the temporary SQLite file of the run.
// The steps share one session and run in order; each step depends on the rows the previous ones saved.

import assert from "node:assert/strict";
import { holdWriteLock, queryScalar } from "../support/database.mjs";
import { captureFailure, closeSession, openSession } from "../support/session.mjs";
import {
  absent,
  assertNoCanary,
  byTestId,
  click,
  fill,
  openSection,
  pageText,
  pathForms,
  shown,
  waitForText,
  waitForValue,
} from "../support/ui.mjs";

const PROFILE_NAME = "Ada Example";
const PROFILE_EMAIL = "ada.example@example.test";
const RENAMED_PROFILE = "Ada Example-Lovelace";
const JOB_POSTING = [
  "Senior Rust Engineer",
  "Example Works GmbH",
  "Build local-first desktop tools in Rust with SQLite and Tauri.",
  "Required skills: Rust, SQLite, Tauri, SQL.",
].join("\n");
const BUSY_POSTING = [
  "Busy Check Engineer",
  "Example Works GmbH",
  "Hold the database write lock while a posting is saved.",
].join("\n");

describe("JobniacFast desktop workflows (default session)", function () {
  this.timeout(180_000);

  let session;
  let browser;
  const canaries = [];

  before(async () => {
    session = await openSession(process.env);
    browser = session.browser;
    // The canaries are the roots the environment names: none of them may ever reach the page.
    const secrets = [
      process.env.JOBNIAC_DOCUMENT_IMPORT_ROOT,
      process.env.JOBNIAC_E2E_GENERATED_ROOT,
      process.env.JOBNIAC_DB_PATH,
    ];
    for (const root of secrets) {
      if (root) {
        canaries.push(...pathForms(root));
      }
    }
  });

  after(async () => {
    if (session) {
      await closeSession(session);
    }
  });

  afterEach(async function () {
    if (this.currentTest?.state === "failed" && browser) {
      const path = await captureFailure(browser, this.currentTest.title);
      if (path) {
        console.error(`screenshot: ${path}`);
      }
      console.error(`tauri-driver: ${session.driverLog.join("").slice(-1500)}`);
    }
  });

  it("1 starts and the health view shows UP", async () => {
    await openSection(browser, "health");
    const refresh = (await shown(browser, "healthToolbar")).$("button");
    await refresh.waitForClickable({ timeout: 20_000 });
    await refresh.click();
    await browser.$('p[role="status"][data-state="up"]').waitForDisplayed({
      timeout: 60_000,
      timeoutMsg: "the health check did not report UP",
    });
  });

  it("2 creates a profile with an experience, saves it and finds it by search", async () => {
    await openSection(browser, "profiles");
    await click(browser, "newProfileButton");
    await fill(browser, "fullNameInput", PROFILE_NAME);
    await fill(browser, "emailInput", PROFILE_EMAIL);
    await fill(browser, "summaryInput", "Synthetic candidate for the end-to-end suite.");
    await click(browser, "addExperiencesButton");
    await fill(browser, "experiences_0_companyInput", "Example Works GmbH");
    await fill(browser, "experiences_0_titleInput", "Platform Engineer");
    await click(browser, "saveProfileButton");
    await waitForText(browser, "profileMessage", "Profile created.");

    await fill(browser, "nationalityInput", "Synthetic");
    await click(browser, "savePersonalDetailsButton");
    await waitForText(browser, "profileMessage", "Personal details saved.");

    await click(browser, "profileRefreshButton");
    await waitForText(browser, "profileListPanel", PROFILE_NAME);

    await fill(browser, "profileSearchInput", "Example");
    await click(browser, "searchProfilesButton");
    await shown(browser, "openSearchResultButton");
  });

  it("2 edits the profile and saves it against the current revision", async () => {
    await click(browser, "openSearchResultButton");
    await waitForValue(browser, "fullNameInput", PROFILE_NAME);
    await fill(browser, "fullNameInput", RENAMED_PROFILE);
    await click(browser, "saveProfileButton");
    await waitForText(browser, "profileMessage", "Profile saved.");
    await click(browser, "profileRefreshButton");
    await waitForText(browser, "profileListPanel", RENAMED_PROFILE);
  });

  it("2 an unsaved edit opens the discard prompt and discarding restores the saved profile", async () => {
    await fill(browser, "fullNameInput", "Unsaved Edit");
    await click(browser, "reloadProfileButton");
    await shown(browser, "unsavedChangesDialog");
    await waitForText(browser, "unsavedChangesMessage", "Discard your unsaved edits");
    await click(browser, "confirmUnsavedActionButton");
    await absent(browser, "unsavedChangesDialog");
    assert.equal(await byTestId(browser, "fullNameInput").getValue(), RENAMED_PROFILE);
  });

  it("3 pastes a job posting and sees it in the saved list", async () => {
    await openSection(browser, "jobs");
    // The paste panel is hidden until "Add posting" is pressed.
    await click(browser, "addPostingToggleButton");
    await fill(browser, "jobTextInput", JOB_POSTING);
    await click(browser, "addJobButton");
    await waitForText(browser, "jobSaveStatusLabel", "Job posting saved.");
    await click(browser, "refreshJobsButton");
    await waitForText(browser, "jobListPanel", "Senior Rust Engineer");
  });

  it("4 analyzes the profile and job pair and opens the match report", async () => {
    const profileId = queryScalar(
      "select id from profile_profiles where email = ?",
      process.env,
      PROFILE_EMAIL,
    );
    const jobId = queryScalar(
      // A prefix match: a WebDriver that types the newlines as nothing turns the whole posting
      // into the first line, which becomes the title.
      "select id from job_jobs where title like 'Senior Rust Engineer%'",
      process.env,
    );
    await openSection(browser, "matching");
    await fill(browser, "matchingProfileIdInput", profileId);
    await fill(browser, "matchingJobIdInput", jobId);
    await click(browser, "analyzeJobMatchButton");
    await shown(browser, "matchReportScoreLabel");
    await click(browser, "loadMatchReportsButton");
    await click(browser, "openMatchReportButton");
    await shown(browser, "matchReportAlgorithmLabel");
  });

  it.skip("5 imports a PDF through the Documents screen", async () => {
    // Not automated: the file is chosen in the native open dialog, which WebDriver cannot drive, and
    // PdfPicker has no test seam in the desktop shell (src-tauri/src/document_commands.rs). The
    // import root only sets the dialog's start folder. See e2e/README.md.
  });

  it("7 the opt-in screens are absent when their flags are unset", async () => {
    // A section mounts on its first visit, so each control is checked in the section that owns it.
    const ownerSection = {
      arbeitnowScanButton: "arbeitnow",
      addJobFromLinkButton: "jobs",
      deleteProfileButton: "profiles",
      deleteJobButton: "jobs",
      previewArtifactButton: "artifacts",
      generatePdfButton: "generation",
    };
    for (const testId of [
      "arbeitnowScanButton",
      "addJobFromLinkButton",
      "deleteProfileButton",
      "deleteJobButton",
    ]) {
      await openSection(browser, ownerSection[testId]);
      await absent(browser, testId);
    }
    // The artifact and generation controls are rendered disabled when their flags are unset (see
    // e2e/README.md): no opt-in control may be enabled.
    for (const testId of ["previewArtifactButton", "generatePdfButton"]) {
      await openSection(browser, ownerSection[testId]);
      const control = byTestId(browser, testId);
      if (await control.isExisting()) {
        assert.equal(
          await control.isEnabled(),
          false,
          `${testId} is enabled while its flag is unset`,
        );
      }
    }
  });

  it("8 a busy save disables its control until the database releases the write lock", async () => {
    // A held write lock makes the save of a posting wait in the database (the busy timeout is ten
    // seconds, longer than this check). The lock is released by ending the holder, so the busy
    // window is deterministic.
    await openSection(browser, "jobs");
    if (!(await byTestId(browser, "jobTextInput").isDisplayed())) {
      await click(browser, "addPostingToggleButton");
    }
    await fill(browser, "jobTextInput", BUSY_POSTING);
    const add = await shown(browser, "addJobButton");
    const release = holdWriteLock(process.env);
    try {
      await add.click();
      await browser.waitUntil(async () => !(await add.isEnabled()), {
        timeout: 10_000,
        timeoutMsg: "the save control stayed enabled while the save ran",
      });
    } finally {
      release();
    }
    await click(browser, "refreshJobsButton");
    // The save finished once the lock was gone: the posting is in the list.
    await waitForText(browser, "jobListPanel", "Busy Check Engineer");
  });

  it("8 no file path or token appears in the page", async () => {
    await assertNoCanary(browser, canaries);
    const text = await pageText(browser);
    assert.equal(text.includes("jobniac.sqlite3"), false);
  });

  it.skip("8 a window close with unsaved edits is refused until the discard is confirmed", async () => {
    // Not automated: WebDriver's window close destroys the window without the CloseRequested event,
    // so the Rust close gate is not reached (the session ended with the window and the gate did not
    // refuse). The discard prompt itself is covered by "2 an unsaved edit opens the discard prompt".
  });
});
