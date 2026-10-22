import { expect, type Page, test } from "./fixtures";

const postings = (page: Page) => page.getByRole("list", { name: "Your postings" });

test.describe("saved jobs", () => {
  test("starts unloaded, then lists the saved postings with a paging control", async ({
    page,
    open,
  }) => {
    await open("jobs");
    await expect(page.getByText("No saved jobs loaded.")).toBeVisible();
    await page.getByRole("button", { name: "Refresh saved jobs" }).click();
    await expect(postings(page).getByRole("listitem")).toHaveCount(10);
    await expect(postings(page)).toContainText("Lumenwork GmbH");
    await expect(page.getByRole("button", { name: "Next jobs page" })).toBeVisible();
  });

  test("opening a posting shows its skills and description", async ({ page, open }) => {
    await open("jobs");
    await page.getByRole("button", { name: "Refresh saved jobs" }).click();
    await page.getByRole("button", { name: /Open saved job Senior Backend Engineer/ }).click();
    const detail = page.getByRole("region", { name: "Senior Backend Engineer (Rust)" });
    await expect(detail).toBeVisible();
    await expect(detail.getByRole("list", { name: /Skills/ })).toContainText("PostgreSQL");
    await expect(detail.getByRole("region", { name: "Job description" })).toContainText(
      "event ingestion platform",
    );
  });

  test("Search jobs needs a query and narrows the list", async ({ page, open }) => {
    await open("jobs");
    const search = page.getByRole("textbox", { name: "Search saved jobs" });
    const button = page.getByRole("button", { name: "Search jobs" });
    await expect(button).toBeDisabled();
    await search.fill("Kubernetes");
    await expect(button).toBeEnabled();
    await button.click();
    await expect(
      page.getByRole("button", { name: /Open saved job DevOps Engineer/ }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /Open saved job Embedded Software Engineer/ }),
    ).toHaveCount(0);
  });

  test("an empty store lists nothing", async ({ page, open }) => {
    await open("jobs", { mockEmpty: "1" });
    await page.getByRole("button", { name: "Refresh saved jobs" }).click();
    await expect(postings(page).getByRole("listitem")).toHaveCount(0);
  });
});

test.describe("add a posting", () => {
  test("the form is hidden until Add posting is pressed", async ({ page, open }) => {
    await open("jobs");
    await expect(page.getByRole("region", { name: "Save a job posting" })).toHaveCount(0);
    await page.getByRole("button", { name: "Add posting" }).click();
    await expect(page.getByRole("region", { name: "Save a job posting" })).toBeVisible();
  });

  test("Save posting needs text and then confirms the save", async ({ page, open }) => {
    await open("jobs");
    await page.getByRole("button", { name: "Add posting" }).click();
    const save = page.getByRole("button", { name: "Save posting" });
    const text = page.getByRole("textbox", { name: "Job posting text" });
    await expect(save).toBeDisabled();
    await text.fill("Senior Rust Engineer\nExample Corp, Berlin\nRust, PostgreSQL, Kubernetes");
    await expect(save).toBeEnabled();
    await save.click();
    await expect(page.getByTestId("jobSaveStatusLabel")).toHaveText("Job posting saved.");
    await expect(text).toHaveValue("");
  });

  test("Add from link needs a link", async ({ page, open }) => {
    await open("jobs");
    await page.getByRole("button", { name: "Add posting" }).click();
    const add = page.getByRole("button", { name: "Add from link" });
    await expect(add).toBeDisabled();
    await page.getByRole("textbox", { name: "Job link" }).fill("https://example.org/jobs/1");
    await expect(add).toBeEnabled();
  });
});
