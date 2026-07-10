import { expect, test } from "@playwright/test";

const middlewareEnabled = process.env.MIDDLEWARE === "1";

test("SSR shell contains the unhydrated counter island", async ({ request }) => {
  const response = await request.get("/");
  expect(response.ok()).toBeTruthy();

  const html = await response.text();
  expect(html).toContain("<leptos-island");
  expect(html).toContain("data-component=");
  expect(html).toContain("Increment Counter");
  expect(html).toContain("COUNT VALUE");
});

test("component middleware preserves public split assets", async ({ request }) => {
  test.skip(!middlewareEnabled, "requires the composed middleware chain");

  const pageResponse = await request.get("/");
  expect(pageResponse.ok()).toBeTruthy();
  expect(pageResponse.headers()["x-request-id"]).toBeTruthy();
  expect(pageResponse.headers()["x-content-type-options"]).toBe("nosniff");
  expect(pageResponse.headers()["referrer-policy"]).toBe(
    "strict-origin-when-cross-origin",
  );

  // Spin serves this through a separate public file-service trigger, whereas
  // Wasmtime reaches the Leptos static callback inside the composed service.
  // Both paths must remain available without authentication for hydration.
  const loaderResponse = await request.get("/pkg/counter.js");
  expect(loaderResponse.ok()).toBeTruthy();
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

  const button = page.locator('button[type="button"]');
  const acceptedAction = page.waitForResponse(/\/api\/increment_count(?:\?|$)/);
  await button.click();
  await expect(button).toBeDisabled();
  await expect(button).toHaveText("Updating...");
  const acceptedResponse = await acceptedAction;
  expect(acceptedResponse.status()).toBe(200);
  await expect(count).toHaveText("1");
  await expect(button).toBeEnabled();
  await expect(button).toHaveText("Increment Counter");
  if (middlewareEnabled) {
    expect(acceptedResponse.headers()["x-request-id"]).toBeTruthy();
    expect(acceptedResponse.headers()["x-content-type-options"]).toBe("nosniff");
  }
});
