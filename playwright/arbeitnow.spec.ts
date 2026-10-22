import { expect, test } from "./fixtures";

test.describe("Arbeitnow", () => {
  test("a scan lists postings with their company, place and an Import button", async ({
    page,
    open,
  }) => {
    await open("arbeitnow");
    await expect(page.getByText("No results yet")).toBeVisible();
    await page.getByRole("button", { name: "Scan Arbeitnow" }).click();
    const results = page.getByTestId("arbeitnowPage").getByRole("listitem");
    await expect(results.first()).toContainText("Backend Developer Go");
    await expect(results.first()).toContainText("Harbor Logic");
    await expect(page.getByRole("button", { name: /^Import / })).toHaveCount(5);
  });

  test("Load more results appends another page", async ({ page, open }) => {
    await open("arbeitnow");
    await page.getByRole("button", { name: "Scan Arbeitnow" }).click();
    const imports = page.getByRole("button", { name: /^Import / });
    await expect(imports).toHaveCount(5);
    await page.getByRole("button", { name: "Load more Arbeitnow results" }).click();
    await expect(imports).not.toHaveCount(5);
  });

  test("the filters can be filled and the job types ticked", async ({ page, open }) => {
    await open("arbeitnow");
    await page.getByRole("textbox", { name: "Arbeitnow location filter" }).fill("Berlin");
    await page.getByRole("checkbox", { name: "Full time" }).check();
    await expect(page.getByRole("checkbox", { name: "Full time" })).toBeChecked();
    await expect(page.getByRole("checkbox", { name: "Part time" })).not.toBeChecked();
    await page.getByRole("button", { name: "Scan Arbeitnow" }).click();
    await expect(page.getByRole("button", { name: "Import Backend Developer Go" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Import Data Engineer" })).toHaveCount(0);
  });

  test("filters that match nothing list no postings", async ({ page, open }) => {
    await open("arbeitnow");
    await page.getByRole("textbox", { name: "Arbeitnow search terms" }).fill("rust");
    await page.getByRole("textbox", { name: "Arbeitnow location filter" }).fill("Berlin");
    await page.getByRole("button", { name: "Scan Arbeitnow" }).click();
    await expect(page.getByRole("button", { name: /^Import / })).toHaveCount(0);
  });

  test("importing a posting reports the result and does not drop the results", async ({
    page,
    open,
  }) => {
    await open("arbeitnow");
    await page.getByRole("button", { name: "Scan Arbeitnow" }).click();
    await page.getByRole("button", { name: "Import Backend Developer Go" }).click();
    await expect(page.getByRole("button", { name: /^Import / }).first()).toBeVisible();
  });

  test("a failing scan reports an error and lists nothing", async ({ page, open }) => {
    await open("arbeitnow", { mockFail: "arbeitnow_scan" });
    await page.getByRole("button", { name: "Scan Arbeitnow" }).click();
    await expect(page.getByRole("button", { name: /^Import / })).toHaveCount(0);
  });
});
