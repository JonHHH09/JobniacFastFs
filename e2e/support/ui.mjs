// Page helpers. Every wait is an explicit condition with a timeout; nothing sleeps.
import assert from "node:assert/strict";

export const WAIT = 20_000;

/** The element with the given data-testid. */
export function byTestId(browser, testId) {
  return browser.$(`[data-testid="${testId}"]`);
}

/** Waits until the element with the testid is displayed, then returns it. */
export async function shown(browser, testId, timeout = WAIT) {
  const element = byTestId(browser, testId);
  await element.waitForDisplayed({ timeout, timeoutMsg: `${testId} was not shown` });
  return element;
}

/** Waits until the element with the testid is gone or never appears. */
export async function absent(browser, testId, timeout = 3_000) {
  await browser.waitUntil(async () => !(await byTestId(browser, testId).isExisting()), {
    timeout,
    timeoutMsg: `${testId} is still in the page`,
  });
}

/** Clicks a control once it is displayed and enabled. */
export async function click(browser, testId, timeout = WAIT) {
  const element = await shown(browser, testId, timeout);
  await browser.waitUntil(() => element.isEnabled(), {
    timeout,
    timeoutMsg: `${testId} stayed disabled`,
  });
  await element.click();
}

/**
 * Opens a section of the shell through its navigation item and waits until its panel is
 * shown. A section mounts on its first visit and stays mounted, so a later call only switches.
 * The ids are the `?section=` values: profiles, jobs, arbeitnow, matching, reviews,
 * documents, artifacts, generation and health.
 */
export async function openSection(browser, id) {
  await click(browser, `nav-${id}`);
  await shown(browser, `section-${id}`);
}

/** Replaces the value of an input the way a user does (select all, type). */
export async function fill(browser, testId, value) {
  const element = await shown(browser, testId);
  await element.clearValue();
  await element.setValue(value);
}

/** The visible text of the whole page. */
export async function pageText(browser) {
  return browser.execute(() => document.body.innerText);
}

/** Every attribute value and text node of the page, joined: used to search for leaked values. */
export async function pageSurface(browser) {
  return browser.execute(() => {
    const parts = [document.body.innerText];
    for (const element of document.querySelectorAll("*")) {
      for (const attribute of element.attributes) {
        parts.push(attribute.value);
      }
    }
    return parts.join("\n");
  });
}

/** Waits until the input with the testid holds the expected value. */
export async function waitForValue(browser, testId, expected, timeout = WAIT) {
  const element = await shown(browser, testId, timeout);
  await browser.waitUntil(async () => (await element.getValue()) === expected, {
    timeout,
    timeoutMsg: `${testId} did not hold "${expected}"`,
  });
}

/** Waits until the element with the testid contains the expected text. */
export async function waitForText(browser, testId, expected, timeout = WAIT) {
  const element = await shown(browser, testId, timeout);
  await browser.waitUntil(async () => (await element.getText()).includes(expected), {
    timeout,
    timeoutMsg: `${testId} did not show "${expected}"`,
  });
}

/** Asserts that no canary string (a path or a token) occurs in the page text or attributes. */
export async function assertNoCanary(browser, canaries) {
  const surface = await pageSurface(browser);
  for (const canary of canaries) {
    assert.equal(surface.includes(canary), false, "a file path or token reached the page");
  }
}

/** Renders the path with both separators, the forms a leak could take on Windows. */
export function pathForms(path) {
  return [path, path.replaceAll("\\", "/"), path.replaceAll("/", "\\")];
}
