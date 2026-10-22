import { expect, type Page, test } from "./fixtures";

async function pickPair(page: Page) {
  await page.getByRole("combobox", { name: "Profile" }).click();
  await page.getByRole("option", { name: /Mara Teller/ }).click();
  await page.getByRole("combobox", { name: "Job" }).click();
  await page.getByRole("option", { name: /Senior Backend Engineer/ }).click();
}

test.describe("matching", () => {
  test("Compare needs both a profile and a job", async ({ page, open }) => {
    await open("matching");
    const compare = page.getByRole("button", { name: "Compare this pair" });
    await expect(compare).toBeDisabled();
    await expect(page.getByTestId("matchingPairHint")).toHaveText(
      "Pick a profile and a job to compare them.",
    );
    await page.getByRole("combobox", { name: "Profile" }).click();
    await page.getByRole("option", { name: /Mara Teller/ }).click();
    await expect(compare).toBeDisabled();
    await page.getByRole("combobox", { name: "Job" }).click();
    await page.getByRole("option", { name: /Senior Backend Engineer/ }).click();
    await expect(compare).toBeEnabled();
  });

  test("the profile list offers the saved profiles", async ({ page, open }) => {
    await open("matching");
    await page.getByRole("combobox", { name: "Profile" }).click();
    await expect(page.getByRole("option")).toHaveCount(5);
    await expect(page.getByRole("option", { name: /Priya Raman/ })).toBeVisible();
  });

  test("comparing a pair shows the score, the components and the evidence", async ({
    page,
    open,
  }) => {
    await open("matching");
    await pickPair(page);
    await page.getByRole("button", { name: "Compare this pair" }).click();
    const report = page.getByRole("region", { name: "Comparison report" });
    await expect(report).toBeVisible();
    await expect(report.getByRole("meter", { name: "Score 92 of 100" })).toBeVisible();
    await expect(report.getByText("Strong match")).toBeVisible();
    for (const component of [
      "Technical skills",
      "Experience and seniority",
      "Domain",
      "Delivery",
      "Hard requirements",
    ]) {
      await expect(report.getByRole("meter", { name: component })).toBeVisible();
    }
    await expect(report).toContainText("Profile lists Rust, which the posting asks for.");
    await expect(report).toContainText("The saved inputs match the versions in this report.");
  });

  test("the report history loads on request", async ({ page, open }) => {
    await open("matching");
    await expect(
      page.getByText("Load the report history to list saved reports here."),
    ).toBeVisible();
    await page.getByRole("button", { name: "Load report history" }).click();
    await expect(page.getByText("Load the report history to list saved reports here.")).toHaveCount(
      0,
    );
  });

  test("a failing comparison reports an error and no report", async ({ page, open }) => {
    await open("matching", { mockFail: "match_analyze_pair" });
    await pickPair(page);
    await page.getByRole("button", { name: "Compare this pair" }).click();
    await expect(page.getByRole("region", { name: "Comparison report" })).toHaveCount(0);
  });
});

test.describe("reviews", () => {
  test("the form is disabled until a report is chosen", async ({ page, open }) => {
    await open("reviews");
    await expect(page.getByRole("button", { name: "Save advisory review" })).toBeDisabled();
    await expect(page.getByRole("button", { name: "Add component detail" })).toBeDisabled();
    await expect(page.getByRole("button", { name: "Discard review draft" })).toBeDisabled();
    await expect(page.getByTestId("reviewAuthorityLabel")).toBeVisible();
  });

  test("the outcome and summary choices are offered", async ({ page, open }) => {
    await open("reviews");
    await expect(page.getByRole("combobox", { name: "Outcome" }).locator("option")).toHaveText([
      "Strong match",
      "Partial match",
      "Weak match",
      "Insufficient evidence",
    ]);
  });

  test("saved reviews and disagreements load on request", async ({ page, open }) => {
    await open("reviews");
    await page.getByRole("button", { name: "Load reviews" }).click();
    await page.getByRole("button", { name: "Load disagreements" }).click();
    await expect(page.getByTestId("reviewListsPanel")).toBeVisible();
  });
});
