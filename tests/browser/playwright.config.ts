import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "counter.spec.ts",
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: process.env.BASE_URL ?? "http://127.0.0.1:3000",
    trace: "retain-on-failure",
  },
});
