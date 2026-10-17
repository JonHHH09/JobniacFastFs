// The generation session: generation is enabled with an owned generated root in a temporary
// directory. It runs after the default session, on the same disposable database.
import { captureFailure, closeSession, openSession } from "../support/session.mjs";
import {
  assertNoCanary,
  click,
  fill,
  openSection,
  pathForms,
  shown,
  waitForText,
} from "../support/ui.mjs";

describe("JobniacFast standalone generation (generation session)", function () {
  this.timeout(180_000);

  let session;
  let browser;
  const canaries = [];

  before(async () => {
    session = await openSession(process.env);
    browser = session.browser;
    const secrets = [
      process.env.JOBNIAC_DOCUMENT_IMPORT_ROOT,
      process.env.JOBNIAC_E2E_GENERATED_ROOT,
      process.env.JOBNIAC_DB_PASSWORD,
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
      await captureFailure(browser, this.currentTest.title);
    }
  });

  it("6 generates a standalone PDF with generation enabled and shows the output rows", async () => {
    await openSection(browser, "generation");
    await shown(browser, "generationPage");
    // The standalone kind takes a file name, a title and a body (no profile or job identifiers).
    await fill(browser, "generationFileName", "synthetic-standalone");
    await fill(browser, "generationTitle", "Synthetic standalone document");
    await fill(browser, "generationBody", "Synthetic body text for the end-to-end suite.");
    await click(browser, "generatePdfButton");
    // The output rows appear once the PDF is generated: the summary names its size and page count.
    await waitForText(browser, "generationSummary", "PDF generated", 90_000);
    await shown(browser, "generationOutputChoice");
    await assertNoCanary(browser, canaries);
  });
});
