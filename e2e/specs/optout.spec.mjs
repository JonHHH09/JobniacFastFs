// The opt-out session: the runner sets every feature flag to `0`, which wins over the Settings
// switches, so the optional controls must be off. It runs after the default session on the same
// temporary database file (the profile and the job it saved are still there) and only reads.
//
// Every absence check first waits for a positive signal that the decision was made: the Tauri
// capability command reports the feature unavailable. A control that renders only after an async
// capability query would otherwise pass `absent` before the query resolved.
import assert from "node:assert/strict";
import { captureFailure, closeSession, openSession } from "../support/session.mjs";
import { absent, click, fill, openSection, shown, WAIT } from "../support/ui.mjs";

/** Calls a Tauri command through the app's IPC and returns `{ value }` or `{ error }`. */
async function invoke(browser, command) {
  // WebdriverIO 9+ has no executeAsync: execute awaits the promise the script returns.
  return browser.execute(
    (name) =>
      window.__TAURI_INTERNALS__.invoke(name).then(
        (value) => ({ value }),
        (error) => ({ error: error?.message ?? JSON.stringify(error) }),
      ),
    command,
  );
}

/** Waits until the capability command answers, and asserts that `field` is `false`. */
async function assertUnavailable(browser, command, field) {
  let answer;
  await browser.waitUntil(
    async () => {
      answer = await invoke(browser, command);
      return answer.error === undefined;
    },
    { timeout: WAIT, timeoutMsg: `${command} did not answer` },
  );
  assert.equal(answer.value[field], false, `${command} reports ${field} while its flag is 0`);
}

async function assertDisabled(browser, testId) {
  const control = await shown(browser, testId);
  assert.equal(await control.isEnabled(), false, `${testId} is enabled while its flag is 0`);
}

describe("JobniacFast optional features (opt-out session)", function () {
  this.timeout(180_000);

  let session;
  let browser;

  before(async () => {
    session = await openSession(process.env);
    browser = session.browser;
  });

  after(async () => {
    if (session) {
      await closeSession(session);
    }
  });

  afterEach(async function () {
    if (this.currentTest?.state === "failed" && browser) {
      await captureFailure(browser, this.currentTest.title);
    }
  });

  // A section mounts on its first visit, so each control is checked in the section that owns it.
  it("7 the Arbeitnow scan control is absent when its flag is 0", async () => {
    await assertUnavailable(browser, "arbeitnow_capabilities_get", "arbeitnow");
    await openSection(browser, "arbeitnow");
    await absent(browser, "arbeitnowScanButton");
  });

  it("7 the add-job-from-link control is absent when its flag is 0", async () => {
    await assertUnavailable(browser, "job_link_capabilities_get", "jobLink");
    await openSection(browser, "jobs");
    // The link row sits in the paste panel, which is hidden until "Add posting" is pressed.
    await click(browser, "addPostingToggleButton");
    await shown(browser, "jobTextInput");
    await absent(browser, "addJobFromLinkButton");
  });

  it("7 the delete controls are absent in the profile and job editors when deletion is 0", async () => {
    await assertUnavailable(browser, "deletion_capabilities_get", "deletion");
    // The delete buttons render only in edit mode: open the saved profile and the saved job.
    await openSection(browser, "profiles");
    await fill(browser, "profileSearchInput", "Example");
    await click(browser, "searchProfilesButton");
    await click(browser, "openSearchResultButton");
    await shown(browser, "reloadProfileButton");
    await absent(browser, "deleteProfileButton");

    await openSection(browser, "jobs");
    await click(browser, "openJobButton");
    await shown(browser, "reloadJobButton");
    await absent(browser, "deleteJobButton");
  });

  it("7 the artifact and generation controls are disabled when their flags are 0", async () => {
    await assertUnavailable(browser, "artifact_capabilities_get", "artifacts");
    await assertUnavailable(browser, "generation_capabilities_get", "generation");
    await openSection(browser, "artifacts");
    await assertDisabled(browser, "previewArtifactButton");
    await openSection(browser, "generation");
    await assertDisabled(browser, "generatePdfButton");
  });
});
