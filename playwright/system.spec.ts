import { expect, test } from "./fixtures";

test.describe("health", () => {
  test("starts unchecked and reports Up after a refresh", async ({ page, open }) => {
    await open("health");
    const result = page.getByTestId("healthResult");
    await expect(result.getByRole("status")).toHaveAttribute("data-state", "none");
    await page.getByRole("button", { name: "Refresh health" }).click();
    await expect(result.getByRole("status")).toHaveAttribute("data-state", "up");
    const details = page.getByTestId("healthCheckDetails");
    await expect(details.locator("dt:has-text('Successful checks') + dd")).toHaveText("3");
    await expect(details.locator("dt:has-text('Failed checks') + dd")).toHaveText("0");
  });

  test("a failing health command shows Down with the reason", async ({ page, open }) => {
    await open("health", { mockFail: "health_check" });
    await page.getByRole("button", { name: "Refresh health" }).click();
    const result = page.getByTestId("healthResult");
    await expect(result.getByRole("status")).toHaveAttribute("data-state", "down");
    await expect(result).toContainText("The health check failed");
  });
});

test.describe("settings", () => {
  test("the MCP server switch goes from stopped through starting to running", async ({
    page,
    open,
  }) => {
    await open("settings");
    const status = page.getByTestId("mcpStatus");
    const toggle = page.getByRole("switch", { name: "Run the MCP server" });
    await expect(status).toContainText("Stopped");
    await toggle.click();
    await expect(status).toContainText("Starting");
    await expect(status).toContainText("Running", { timeout: 5_000 });
    await toggle.click();
    await expect(status).toContainText("Stopped");
  });

  test("a port in use makes the server fail to start", async ({ page, open }) => {
    await open("settings");
    await page.getByRole("textbox", { name: "Port" }).fill("9999");
    await page.getByRole("button", { name: "Save port" }).click();
    await page.getByRole("switch", { name: "Run the MCP server" }).click();
    await expect(page.getByTestId("mcpStatus")).toContainText(/failed/i, { timeout: 5_000 });
  });

  test("an out-of-range port is flagged and cannot be saved", async ({ page, open }) => {
    await open("settings");
    const port = page.getByRole("textbox", { name: "Port" });
    const save = page.getByRole("button", { name: "Save port" });
    await expect(save).toBeDisabled();
    await port.fill("80");
    await expect(port).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByText("Enter a port from 1024 to 65535.")).toBeVisible();
    await expect(save).toBeDisabled();
    await port.fill("9000");
    await expect(port).not.toHaveAttribute("aria-invalid", "true");
    await expect(save).toBeEnabled();
  });

  test("the update check says when the app is current", async ({ page, open }) => {
    await open("settings");
    await page.getByTestId("updateCheckButton").click();
    await expect(page.getByTestId("updatesCard").getByRole("status")).toHaveText(
      "You have the latest version.",
    );
  });

  test("the update check announces a newer version", async ({ page, open }) => {
    await open("settings", { mockUpdate: "1" });
    await page.getByTestId("updateCheckButton").click();
    await expect(page.getByTestId("updatesCard")).toContainText("9.9.9");
  });
});
