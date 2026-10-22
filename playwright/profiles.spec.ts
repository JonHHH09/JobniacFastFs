import { expect, type Page, test } from "./fixtures";

const savedProfiles = (page: Page) => page.getByRole("list", { name: "Saved profiles" });
const workspace = (page: Page) => page.getByRole("region", { name: "Profile workspace" });

async function openMara(page: Page) {
  await page.getByRole("button", { name: "Refresh profiles" }).click();
  await savedProfiles(page)
    .getByRole("button", { name: /Mara Teller/ })
    .click();
  await expect(page.getByRole("textbox", { name: "Full name" })).toHaveValue("Mara Teller");
}

test.describe("profile list and search", () => {
  test("starts empty until refreshed, then lists the saved profiles", async ({ page, open }) => {
    await open("profiles");
    await expect(page.getByText("Refresh to load profiles.")).toBeVisible();
    await page.getByRole("button", { name: "Refresh profiles" }).click();
    await expect(savedProfiles(page).getByRole("listitem")).toHaveCount(5);
    await expect(savedProfiles(page)).toContainText("jonas.kellner@example.com");
  });

  test("a search narrows to the matching profile and says why it matched", async ({
    page,
    open,
  }) => {
    await open("profiles");
    await page.getByRole("textbox", { name: "Search profiles" }).fill("Rust");
    await page.getByRole("button", { name: "Search", exact: true }).click();
    const results = page.getByRole("list", { name: "Search results" });
    await expect(results.getByRole("listitem")).toHaveCount(1);
    await expect(results).toContainText("Mara Teller");
    await expect(results).toContainText("Matched fields: Summary, Skills");
  });

  test("opening a search result loads it into the workspace", async ({ page, open }) => {
    await open("profiles");
    await page.getByRole("textbox", { name: "Search profiles" }).fill("Rust");
    await page.getByRole("button", { name: "Search", exact: true }).click();
    await page.getByRole("button", { name: "Open profile Mara Teller" }).click();
    await expect(page.getByRole("textbox", { name: "Full name" })).toHaveValue("Mara Teller");
    await expect(page.getByTestId("profileMessage")).toHaveText("Profile loaded.");
  });

  test("an empty store shows the empty list and an injected failure shows an error", async ({
    page,
    open,
  }) => {
    await open("profiles", { mockEmpty: "1" });
    await page.getByRole("button", { name: "Refresh profiles" }).click();
    await expect(savedProfiles(page)).toHaveCount(0);
    await expect(page.getByRole("region", { name: "Saved profiles" })).not.toContainText(
      "Mara Teller",
    );

    await open("profiles", { mockFail: "list_profiles" });
    await page.getByRole("button", { name: "Refresh profiles" }).click();
    await expect(page.getByRole("region", { name: "Saved profiles" })).not.toContainText(
      "Mara Teller",
    );
  });
});

test.describe("profile editor", () => {
  test("an opened profile shows its sections and entries", async ({ page, open }) => {
    await open("profiles");
    await openMara(page);
    for (const name of ["Contacts", "Skills", "Languages", "Education", "Experience", "Projects"]) {
      await expect(workspace(page).getByRole("region", { name, exact: true })).toBeVisible();
    }
    await expect(page.getByRole("textbox", { name: "Company" }).first()).toHaveValue(
      "Lumenwork GmbH",
    );
    await expect(page.getByRole("button", { name: "Save profile" })).toBeDisabled();
  });

  test("an edit enables Save, Save confirms and disables it again", async ({ page, open }) => {
    await open("profiles");
    await openMara(page);
    const summary = page.getByRole("textbox", { name: "Summary" });
    await summary.fill("Backend engineer, now focused on observability.");
    const save = page.getByRole("button", { name: "Save profile" });
    await expect(save).toBeEnabled();
    await save.click();
    await expect(page.getByTestId("profileMessage")).toHaveText("Profile saved.");
    await expect(save).toBeDisabled();
    await expect(summary).toHaveValue("Backend engineer, now focused on observability.");
  });

  test("Discard changes restores the loaded profile after a confirmation", async ({
    page,
    open,
  }) => {
    await open("profiles");
    await openMara(page);
    const name = page.getByRole("textbox", { name: "Full name" });
    await name.fill("Somebody Else");
    await page.getByRole("button", { name: "Discard changes" }).click();
    const dialog = page.getByTestId("unsavedChangesDialog");
    await expect(dialog).toBeVisible();
    await page.getByTestId("confirmUnsavedActionButton").click();
    await expect(dialog).toBeHidden();
    await expect(name).toHaveValue("Mara Teller");
  });

  test("an entry can be added and removed", async ({ page, open }) => {
    await open("profiles");
    await openMara(page);
    const skills = page.getByRole("region", { name: "Skills entries" });
    await expect(skills.getByRole("textbox", { name: "Skill" })).toHaveCount(5);
    await page.getByRole("button", { name: "Add skill" }).click();
    await expect(skills.getByRole("textbox", { name: "Skill" })).toHaveCount(6);
    await page.getByRole("button", { name: "Remove entry 2: Go" }).click();
    await expect(skills.getByRole("textbox", { name: "Skill" })).toHaveCount(5);
    await expect(page.getByRole("button", { name: "Save profile" })).toBeEnabled();
  });
});

test.describe("unsaved edits are guarded", () => {
  test("opening another profile with unsaved edits asks first, and Keep editing keeps them", async ({
    page,
    open,
  }) => {
    await open("profiles");
    await openMara(page);
    const name = page.getByRole("textbox", { name: "Full name" });
    await name.fill("Mara Teller (edited)");

    await savedProfiles(page)
      .getByRole("button", { name: /Jonas Kellner/ })
      .click();
    const dialog = page.getByTestId("unsavedChangesDialog");
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId("unsavedChangesMessage")).toContainText("open the selected");
    await page.getByTestId("cancelUnsavedActionButton").click();
    await expect(dialog).toBeHidden();
    await expect(name).toHaveValue("Mara Teller (edited)");

    await savedProfiles(page)
      .getByRole("button", { name: /Jonas Kellner/ })
      .click();
    await page.getByTestId("confirmUnsavedActionButton").click();
    await expect(name).toHaveValue("Jonas Kellner");
  });

  test("an edited profile marks its navigation item as unsaved", async ({ page, open }) => {
    await open("profiles");
    await openMara(page);
    await expect(page.getByTestId("nav-profiles")).not.toContainText(/unsaved/i);
    await page.getByRole("textbox", { name: "Full name" }).fill("Mara T.");
    await expect(page.getByTestId("nav-profiles")).toContainText(/unsaved/i);
  });

  test("Escape on the confirmation keeps the edits", async ({ page, open }) => {
    await open("profiles");
    await openMara(page);
    const name = page.getByRole("textbox", { name: "Full name" });
    await name.fill("Keep me");
    await page.getByRole("button", { name: "New profile" }).click();
    await expect(page.getByTestId("unsavedChangesDialog")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("unsavedChangesDialog")).toBeHidden();
    await expect(name).toHaveValue("Keep me");
  });
});

test.describe("new profile", () => {
  test("saving an empty form marks the required fields", async ({ page, open }) => {
    await open("profiles");
    await page.getByRole("button", { name: "New profile" }).click();
    await page.getByRole("button", { name: "Save profile" }).click();
    const name = page.getByRole("textbox", { name: "Full name" });
    const email = page.getByRole("textbox", { name: "Email address" });
    await expect(name).toHaveAttribute("aria-invalid", "true");
    await expect(email).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByText("Full name is required.")).toBeVisible();
    await expect(page.getByText("Email address is required.")).toBeVisible();
    await expect(page.getByTestId("profileMessage")).toHaveText(
      "Some profile fields are invalid. Review the form and try again.",
    );
  });

  test("an invalid email is flagged when the field is left", async ({ page, open }) => {
    await open("profiles");
    await page.getByRole("button", { name: "New profile" }).click();
    const email = page.getByRole("textbox", { name: "Email address" });
    await email.fill("not-an-email");
    await email.blur();
    await expect(email).toHaveAttribute("aria-invalid", "true");
  });

  test("a valid profile is created and personal details become editable", async ({
    page,
    open,
  }) => {
    await open("profiles");
    await page.getByRole("button", { name: "New profile" }).click();
    await expect(
      page.getByRole("textbox", { name: "Date of birth (YYYY-MM-DD, optional)" }),
    ).toBeDisabled();
    await page.getByRole("textbox", { name: "Full name" }).fill("Ada Example");
    await page.getByRole("textbox", { name: "Email address" }).fill("ada@example.com");
    await page.getByRole("button", { name: "Save profile" }).click();
    await expect(page.getByTestId("profileMessage")).toHaveText("Profile created.");
    await expect(
      page.getByRole("textbox", { name: "Date of birth (YYYY-MM-DD, optional)" }),
    ).toBeEnabled();

    await page.getByRole("textbox", { name: "Search profiles" }).fill("Ada");
    await page.getByRole("button", { name: "Search", exact: true }).click();
    await expect(page.getByRole("list", { name: "Search results" })).toContainText("Ada Example");
  });
});

test.describe("workspace layout", () => {
  test("the workspace can be hidden and shown again", async ({ page, open }) => {
    await open("profiles");
    const toggle = page.getByTestId("toggleWorkspaceButton");
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await expect(page.getByTestId("profileEditorPanel")).toBeHidden();
    await toggle.click();
    await expect(page.getByTestId("profileEditorPanel")).toBeVisible();
  });
});
