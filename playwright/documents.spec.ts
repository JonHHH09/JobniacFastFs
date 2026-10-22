import { expect, STORED_PDF_ID, test } from "./fixtures";

test.describe("generate", () => {
  test("the PDF type list offers every kind", async ({ page, open }) => {
    await open("generation");
    await expect(page.getByRole("combobox", { name: "PDF type" }).locator("option")).toHaveText([
      "Standalone PDF",
      "Master resume",
      "Canadian resume",
      "French Canadian resume",
      "Germany resume (EN + DE)",
      "German cover letter",
    ]);
  });

  test("an empty body is refused with a message", async ({ page, open }) => {
    await open("generation");
    await page.getByRole("button", { name: "Generate PDF" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Unable to complete" })).toBeVisible();
  });

  test("a standalone PDF is generated and offered for preview and saving", async ({
    page,
    open,
  }) => {
    await open("generation");
    await page.getByTestId("generationTitle").fill("Synthetic note");
    await page.getByTestId("generationBody").fill("Hello from the browser suite.");
    await page.getByRole("button", { name: "Generate PDF" }).click();
    await expect(page.getByText("Generation completed.")).toBeVisible();
    await expect(page.getByText(/PDF generated: .* 1 page/)).toBeVisible();
    await expect(page.getByRole("button", { name: "Preview generated PDF text" })).toBeVisible();
    await expect(page.getByRole("button", { name: "View PDF pages" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Save PDF…" })).toBeVisible();
  });
});

test.describe("stored PDFs", () => {
  test("the picker lists the stored PDFs", async ({ page, open }) => {
    await open("artifacts");
    await page.getByRole("combobox", { name: "Stored PDF" }).click();
    await expect(page.getByRole("option")).toHaveCount(3);
    await expect(page.getByRole("option", { name: /synthetic-cv\.pdf/ })).toBeVisible();
  });

  test("View PDF opens the pages and the viewer pages and zooms", async ({ page, open }) => {
    await open("artifacts");
    await expect(page.getByRole("button", { name: "View PDF" })).toBeDisabled();
    await page.getByRole("combobox", { name: "Stored PDF" }).fill(STORED_PDF_ID);
    await page.getByRole("button", { name: "View PDF" }).click();

    const viewer = page.getByRole("region", { name: "Stored PDF page viewer" });
    await expect(viewer).toBeVisible();
    await expect(viewer.getByText("Page 1 / 2")).toBeVisible();
    await expect(viewer.getByRole("button", { name: "Previous" })).toBeDisabled();
    await viewer.getByRole("button", { name: "Next" }).click();
    await expect(viewer.getByText("Page 2 / 2")).toBeVisible();
    await expect(viewer.getByRole("button", { name: "Next" })).toBeDisabled();
    await viewer.getByRole("radio", { name: "50%" }).check();
    await expect(viewer.getByRole("radio", { name: "50%" })).toBeChecked();
    await expect(viewer.getByRole("img", { name: "PDF page image" })).toBeVisible();
  });
});

test.describe("PDF documents", () => {
  test("the import actions stay disabled until a PDF is chosen", async ({ page, open }) => {
    await open("documents");
    await expect(page.getByRole("button", { name: "Import PDF" })).toBeDisabled();
    await expect(page.getByRole("button", { name: "Preview selected PDF" })).toBeDisabled();
    await expect(page.getByText("No PDF selected")).toBeVisible();
  });

  test("a stored PDF can be picked, its metadata loaded and the view reset", async ({
    page,
    open,
  }) => {
    await open("documents");
    await expect(page.getByRole("button", { name: "Load metadata" })).toBeDisabled();
    await page.getByRole("combobox", { name: "Stored PDF" }).fill(STORED_PDF_ID);
    const metadata = page.getByRole("button", { name: "Load metadata" });
    await expect(metadata).toBeEnabled();
    await metadata.click();
    await expect(page.getByText("No PDF open")).toHaveCount(0);
    await page.getByRole("button", { name: "Reset document view" }).click();
    await expect(page.getByText("No PDF open")).toBeVisible();
  });
});
