import { expect, test } from "@playwright/test";

const middlewareEnabled = process.env.MIDDLEWARE === "1";
const authzEnabled = process.env.AUTHZ === "1";
const cookieSentinel = "browser-cookie-secret-sentinel";
const rawQuerySentinel = "browser_raw_query_secret_sentinel";

function expectTrustedHeadersStripped(headers: Record<string, string>): void {
  expect(headers.authorization).toBeUndefined();
  for (const name of Object.keys(headers)) {
    expect(name.toLowerCase().startsWith("x-wasi-auth-")).toBe(false);
  }
}

test("SSR shell contains the unhydrated counter island", async ({ request }) => {
  const response = await request.get(`/?probe=${rawQuerySentinel}`, {
    headers: { Cookie: `session=${cookieSentinel}` },
  });
  expect(response.ok()).toBeTruthy();
  if (middlewareEnabled) {
    expectTrustedHeadersStripped(response.headers());
  }

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
  expectTrustedHeadersStripped(pageResponse.headers());
  if (authzEnabled) {
    expect(pageResponse.headers()["x-leptos-auth-context-state"]).toBe(
      "anonymous",
    );

    const authenticatedPage = await request.get(`/?probe=${rawQuerySentinel}`, {
      headers: {
        Authorization: "Bearer allow",
        Cookie: `session=${cookieSentinel}`,
        "x-wasi-auth-context": "spoofed-browser-context-sentinel",
      },
    });
    expect(authenticatedPage.ok()).toBeTruthy();
    expect(
      authenticatedPage.headers()["x-leptos-auth-context-state"],
    ).toBe("authenticated");
    expectTrustedHeadersStripped(authenticatedPage.headers());
  }

  // Spin serves this through a separate public file-service trigger, whereas
  // Wasmtime reaches the Leptos static callback inside the composed service.
  // Both paths must remain available without authentication for hydration.
  const loaderResponse = await request.get("/pkg/counter.js");
  expect(loaderResponse.ok()).toBeTruthy();
  expectTrustedHeadersStripped(loaderResponse.headers());
});

test("trusted ingress separates authentication and authorization profiles", async ({ request }) => {
  test.skip(!authzEnabled, "requires the authorization fixture");
  const invoke = async (path: string, credential?: string) => {
    const headers: Record<string, string> = {
      "content-type": "application/x-www-form-urlencoded",
      "x-wasi-auth-context": "spoofed-browser-context-sentinel",
    };
    if (credential) headers.authorization = `Bearer ${credential}`;
    return request.post(path, { headers, data: "current=0" });
  };

  const anonymous = await invoke("/api/authorize_authn");
  expect(anonymous.status()).toBe(401);
  expectTrustedHeadersStripped(anonymous.headers());

  const authenticated = await invoke("/api/authorize_authn", "allow");
  expect(authenticated.status()).toBe(200);
  expectTrustedHeadersStripped(authenticated.headers());

  const cedarDenied = await invoke("/api/authorize_cedar", "readonly");
  expect(cedarDenied.status()).toBe(403);

  const relationshipDenied = await invoke(
    "/api/authorize_relationship",
    "no-relation",
  );
  expect(relationshipDenied.status()).toBe(403);

  const hybridDenied = await invoke("/api/authorize_hybrid_deny", "allow");
  expect(hybridDenied.status()).toBe(403);

  const hybridAllowed = await invoke("/api/increment_count", "allow");
  expect(hybridAllowed.status()).toBe(200);
  for (const response of [
    cedarDenied,
    relationshipDenied,
    hybridDenied,
    hybridAllowed,
  ]) {
    expect(response.headers()["x-request-id"]).toBeTruthy();
    expectTrustedHeadersStripped(response.headers());
  }
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
  await page.route(/\/api\/increment_count(?:\?|$)/, async (route) => {
    // Keep the request pending long enough to observe the UI's disabled state
    // even when the local runtime completes a server function immediately.
    await new Promise((resolve) => setTimeout(resolve, 50));
    await route.continue();
  });
  page.on("response", (response) => {
    const url = response.url();
    if (/\/split_[^/]+\.wasm(?:\?|$)/.test(url)) {
      splitModules.add(url);
    }
  });

  await page.setExtraHTTPHeaders({
    Cookie: `session=${cookieSentinel}`,
  });
  await page.goto(`/?probe=${rawQuerySentinel}`);
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
  if (authzEnabled) {
    const rejectedAction = page.waitForResponse(/\/api\/increment_count(?:\?|$)/);
    await button.click();
    const rejectedResponse = await rejectedAction;
    expect(rejectedResponse.status()).toBe(401);
    expect(rejectedResponse.headers()["x-request-id"]).toBeTruthy();
    expectTrustedHeadersStripped(rejectedResponse.headers());
    await expect(count).toHaveText("0");
    await expect(button).toBeEnabled();

    for (const credential of ["readonly", "lowacr", "no-relation"]) {
      await page.setExtraHTTPHeaders({
        Authorization: `Bearer ${credential}`,
        Cookie: `session=${cookieSentinel}`,
        "x-wasi-auth-context": "spoofed-browser-context-sentinel",
      });
      const deniedAction = page.waitForResponse(
        /\/api\/increment_count(?:\?|$)/,
      );
      await button.click();
      const deniedResponse = await deniedAction;
      expect(deniedResponse.status()).toBe(403);
      expectTrustedHeadersStripped(deniedResponse.headers());
      await expect(count).toHaveText("0");
      await expect(button).toBeEnabled();
    }

    await page.setExtraHTTPHeaders({
      Authorization: "Bearer allow",
      Cookie: `session=${cookieSentinel}`,
      "x-wasi-auth-context": "spoofed-browser-context-sentinel",
    });
  }

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
    expectTrustedHeadersStripped(acceptedResponse.headers());
  }
});
