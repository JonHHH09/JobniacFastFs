import { test as base, expect, type Page } from "@playwright/test";

/** Section ids of the shell, in navigation order (`src/app/sections.tsx`). */
export const SECTION_IDS = [
  "profiles",
  "jobs",
  "arbeitnow",
  "matching",
  "reviews",
  "documents",
  "artifacts",
  "generation",
  "health",
  "settings",
] as const;

export type SectionId = (typeof SECTION_IDS)[number];

/** Synthetic fixtures of the mock IPC (`src/dev/README.md`). */
export const STORED_PDF_ID = "00000000-0000-4000-8000-000600000001";

type Opened = {
  /** Opens a section of the mock app. Latency is 0 unless a `mockLatency` parameter says otherwise. */
  open: (section: SectionId, params?: Record<string, string>) => Promise<void>;
};

export const test = base.extend<Opened & { failOnPageErrors: undefined }>({
  // Every test fails on an uncaught page error or a console error (a missing favicon is ignored).
  failOnPageErrors: [
    async ({ page }, use) => {
      const problems: string[] = [];
      page.on("pageerror", (error) => problems.push(`pageerror: ${error.message}`));
      page.on("console", (message) => {
        if (message.type() === "error" && !message.text().startsWith("Failed to load resource")) {
          problems.push(`console.error: ${message.text()}`);
        }
      });
      await use(undefined);
      expect(problems, "no page errors or console errors").toEqual([]);
    },
    { auto: true },
  ],
  open: async ({ page }, use) => {
    await use(async (section, params = {}) => {
      const query = new URLSearchParams({ section, mockLatency: "0", ...params });
      await page.goto(`/?${query.toString()}`);
      await expect(page.getByTestId(`section-${section}`)).toBeVisible();
    });
  },
});

export { expect, type Page };

/** The main landmark's level-1 heading. */
export function pageHeading(page: Page) {
  return page.getByRole("main").getByRole("heading", { level: 1 });
}
