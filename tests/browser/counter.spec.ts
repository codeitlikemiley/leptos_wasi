import { expect, test } from "@playwright/test";

test("SSR shell contains the unhydrated counter island", async ({ request }) => {
  const response = await request.get("/");
  expect(response.ok()).toBeTruthy();

  const html = await response.text();
  expect(html).toContain("<leptos-island");
  expect(html).toContain("data-component=");
  expect(html).toContain("Increment Counter");
  expect(html).toContain("COUNT VALUE");
});

test("lazy split island loads and handles a server action", async ({ page }) => {
  const splitModules = new Set<string>();
  let splitRequested = false;
  let releaseSplit!: () => void;
  const splitGate = new Promise<void>((resolve) => {
    releaseSplit = resolve;
  });
  await page.route(/\/split_[^/]+\.wasm(?:\?|$)/, async (route) => {
    splitRequested = true;
    await splitGate;
    await route.continue();
  });
  page.on("response", (response) => {
    const url = response.url();
    if (/\/split_[^/]+\.wasm(?:\?|$)/.test(url)) {
      splitModules.add(url);
    }
  });

  await page.goto("/");
  const count = page.locator(".tabular-nums");
  await expect(count).toHaveText("0");
  await expect.poll(() => splitRequested).toBe(true);
  expect(splitModules.size).toBe(0);

  // The server-rendered control is present but cannot dispatch its server
  // action until the lazy island module is allowed to hydrate.
  await page.getByRole("button", { name: "Increment Counter" }).click();
  await expect(count).toHaveText("0");

  const splitFinished = page
    .waitForResponse(/\/split_[^/]+\.wasm(?:\?|$)/)
    .then((response) => response.finished());
  releaseSplit();
  await splitFinished;
  await expect.poll(() => splitModules.size).toBeGreaterThan(0);

  let actionRequested = false;
  let releaseAction!: () => void;
  const actionGate = new Promise<void>((resolve) => {
    releaseAction = resolve;
  });
  await page.route(/\/api\/increment_count(?:\?|$)/, async (route) => {
    actionRequested = true;
    await actionGate;
    await route.continue();
  });

  const button = page.locator('button[type="button"]');
  await button.click();
  await expect.poll(() => actionRequested).toBe(true);
  await expect(button).toBeDisabled();
  await expect(button).toHaveText("Updating...");

  releaseAction();
  await expect(count).toHaveText("1");
  await expect(button).toBeEnabled();
  await expect(button).toHaveText("Increment Counter");
});
