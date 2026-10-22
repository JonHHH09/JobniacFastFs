import { expect, pageHeading, SECTION_IDS, test } from "./fixtures";

const HEADINGS: Record<(typeof SECTION_IDS)[number], string> = {
  profiles: "Profiles",
  jobs: "Saved jobs",
  arbeitnow: "Arbeitnow",
  matching: "Matches",
  reviews: "Reviews",
  documents: "PDF documents",
  artifacts: "Stored PDFs",
  generation: "Generate",
  health: "Health",
  settings: "Settings",
};

test.describe("navigation", () => {
  for (const id of SECTION_IDS) {
    test(`deep link ?section=${id} opens its page`, async ({ page, open }) => {
      await open(id);
      await expect(pageHeading(page)).toHaveText(HEADINGS[id]);
      await expect(page.getByTestId(`nav-${id}`)).toHaveAttribute("aria-current", "page");
    });
  }

  test("an unknown section falls back to Profiles", async ({ page }) => {
    await page.goto("/?section=nope&mockLatency=0");
    await expect(pageHeading(page)).toHaveText("Profiles");
  });

  test("the sidebar switches sections and keeps the address in step", async ({ page, open }) => {
    await open("profiles");
    await page.getByTestId("nav-jobs").click();
    await expect(pageHeading(page)).toHaveText("Saved jobs");
    await expect(page).toHaveURL(/section=jobs/);
    await page.getByTestId("nav-settings").click();
    await expect(pageHeading(page)).toHaveText("Settings");
    await expect(page).toHaveURL(/section=settings/);
  });

  test("Ctrl+number jumps to the nth section", async ({ page, open }) => {
    await open("profiles");
    await page.keyboard.press("Control+3");
    await expect(pageHeading(page)).toHaveText("Arbeitnow");
    await page.keyboard.press("Control+9");
    await expect(pageHeading(page)).toHaveText("Health");
  });

  test("Ctrl+number does not steal the key while typing in a field", async ({ page, open }) => {
    await open("profiles");
    await page.getByRole("textbox", { name: "Search profiles" }).focus();
    await page.keyboard.press("Control+3");
    await expect(pageHeading(page)).toHaveText("Profiles");
  });
});

test.describe("command palette", () => {
  test("Ctrl+K opens it, a query narrows the sections and Enter jumps", async ({ page, open }) => {
    await open("profiles");
    await page.keyboard.press("Control+k");
    const palette = page.getByRole("dialog", { name: "Command palette" });
    await expect(palette).toBeVisible();
    await expect(palette.getByRole("option")).toHaveCount(10);

    await page.getByRole("combobox", { name: "Command palette" }).fill("health");
    await expect(palette.getByRole("option", { name: /^Health/ })).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(palette).toBeHidden();
    await expect(pageHeading(page)).toHaveText("Health");
  });

  test("the sidebar button opens it and Escape closes it", async ({ page, open }) => {
    await open("jobs");
    await page.getByTestId("paletteButton").click();
    const palette = page.getByRole("dialog", { name: "Command palette" });
    await expect(palette).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
    await expect(pageHeading(page)).toHaveText("Saved jobs");
  });

  test("it finds a saved profile by name", async ({ page, open }) => {
    await open("health");
    await page.keyboard.press("Control+k");
    await page.getByRole("combobox", { name: "Command palette" }).fill("Mara");
    await expect(page.getByRole("option", { name: /Mara Teller/ })).toBeVisible();
  });
});

test.describe("theme", () => {
  test("the choice is applied at once and survives a reload", async ({ page, open }) => {
    await open("profiles");
    const root = page.locator("html");
    await page.getByTestId("theme-dark").click();
    await expect(root).toHaveAttribute("data-theme", "dark");
    await expect(page.getByTestId("theme-dark")).toHaveAttribute("aria-pressed", "true");

    await page.reload();
    await expect(root).toHaveAttribute("data-theme", "dark");

    await page.getByTestId("theme-light").click();
    await expect(root).toHaveAttribute("data-theme", "light");
    await page.getByTestId("theme-system").click();
    await expect(root).not.toHaveAttribute("data-theme", /.+/);
  });
});

test.describe("responsive layout", () => {
  test("a middle width collapses the sidebar to an icon rail", async ({ page, open }) => {
    await page.setViewportSize({ width: 800, height: 700 });
    await open("profiles");
    const sidebar = page.getByTestId("sidebar");
    await expect(sidebar).toBeVisible();
    const box = await sidebar.boundingBox();
    expect(box?.width).toBeLessThan(100);
    await expect(page.getByTestId("nav-jobs")).toBeVisible();
  });

  test("a narrow width uses a top bar with a navigation sheet", async ({ page, open }) => {
    await page.setViewportSize({ width: 400, height: 800 });
    await open("profiles");
    await expect(page.getByTestId("sidebar")).toHaveCount(0);
    await page.getByRole("button", { name: /menu|navigation/i }).click();
    await page.getByTestId("nav-health").click();
    await expect(pageHeading(page)).toHaveText("Health");
  });

  test("no section overflows the page horizontally at the narrowest window", async ({
    page,
    open,
  }) => {
    await page.setViewportSize({ width: 360, height: 640 });
    for (const id of SECTION_IDS) {
      await open(id);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow, `${id} overflows by ${overflow}px`).toBeLessThanOrEqual(0);
    }
  });
});
