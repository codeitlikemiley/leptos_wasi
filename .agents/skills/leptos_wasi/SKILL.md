```markdown
# leptos_wasi Development Patterns

> Auto-generated skill from repository analysis

## Overview

This skill provides a comprehensive guide to contributing to the `leptos_wasi` Rust codebase. It covers coding conventions, commit message patterns, testing strategies, and detailed step-by-step workflows for common maintenance and development tasks. Whether you're updating dependencies, adding new test scenarios, or preparing a release, this skill will help you follow the project's established practices.

## Coding Conventions

- **File Naming:**  
  Use `camelCase` for file names.
  ```
  // Good
  src/myModule.rs

  // Bad
  src/my_module.rs
  ```

- **Import Style:**  
  Use relative imports within the crate.
  ```rust
  // Good
  mod utils;
  use crate::utils::do_something;

  // Bad
  use leptos_wasi::utils::do_something;
  ```

- **Export Style:**  
  Default exports are preferred.
  ```rust
  // Good
  pub mod myFeature;

  // Bad
  pub use myFeature::{...};
  ```

- **Commit Messages:**  
  Follow [Conventional Commits](https://www.conventionalcommits.org/). Prefixes include:  
  `test:`, `chore:`, `docs:`, `fix:`, `refactor:`, `perf:`, `ci:`, `feat:`  
  Example:
  ```
  feat: add trusted-ingress test fixture
  fix: correct middleware artifact pinning
  docs: update migration steps for v0.5
  ```

## Workflows

### Release Version Bump
**Trigger:** When preparing a new release or pre-release of leptos_wasi.  
**Command:** `/release-bump`

1. Update the version in `Cargo.toml`.
2. Update the version in `CHANGELOG.md` (if present).
3. Update `Cargo.lock` in the main crate and all test/example fixtures.
4. Commit with a release message.
   ```
   chore: release v0.6.0
   ```
5. Example files to update:
   - `Cargo.toml`
   - `Cargo.lock`
   - `CHANGELOG.md`
   - `examples/counter/Cargo.lock`
   - `tests/api-fixtures/dual-runtime-consumer/Cargo.lock`
   - ... (see full list above)

---

### Add or Update Middleware or Authz Artifacts
**Trigger:** When middleware or authorization artifacts are regenerated, repinned, or updated.  
**Command:** `/refresh-artifacts`

1. Update `tests/middleware/artifact-sets.toml`.
2. Update `tests/middleware/components.lock.toml`.
3. Update `tests/middleware/deployment-policy.toml`.
4. Commit with a pin/refresh/correct message.
   ```
   chore: refresh middleware artifacts for new policy
   ```

---

### Add New Test Fixture or Scenario
**Trigger:** When adding a new test scenario or fixture for middleware, authorization, or trusted-ingress.  
**Command:** `/new-fixture`

1. Create a new test directory (e.g., `tests/authz-lifecycle-wasip2/`).
2. Add `Cargo.toml` and `Cargo.lock`.
3. Add source files (`src/lib.rs` or `src/main.rs`, `README.md`, etc.).
4. Add supporting scripts if needed.
5. Commit with a test: add/cover/prove message.
   ```
   test: add authz-lifecycle-wasip2 fixture
   ```

---

### Add or Update Browser E2E Tests
**Trigger:** When adding or updating browser-based end-to-end tests for new features or middleware changes.  
**Command:** `/e2e-browser`

1. Add or update `tests/browser/*.spec.ts`.
2. Update `tests/browser/run.sh`.
3. Update `tests/browser/package.json` and lockfile if needed.
4. Commit with a test: cover/preserve message.
   ```
   test: cover trusted-ingress with e2e browser spec
   ```

---

### Documentation Update for Feature or Release
**Trigger:** When a new feature, migration, or release requires documentation changes.  
**Command:** `/update-docs`

1. Edit one or more of the following files:
   - `README.md`
   - `MIGRATION.md`
   - `MIDDLEWARE.md`
   - `PERFORMANCE.md`
   - `PRODUCTION.md`
   - `CHANGELOG.md`
2. Commit with a `docs:` message.
   ```
   docs: update production deployment instructions
   ```

---

### CI Policy or Workflow Update
**Trigger:** When CI workflows or dependency policies need to be updated for new requirements.  
**Command:** `/update-ci`

1. Edit `.github/workflows/main.yaml`.
2. Edit `Makefile.toml` or `deny.toml` if needed.
3. Commit with a `ci:` message.
   ```
   ci: add MSRV compatibility check
   ```

---

## Testing Patterns

- **Framework:**  
  [Playwright](https://playwright.dev/) is used for browser-based end-to-end tests.

- **Test File Pattern:**  
  All E2E tests are named with the `.spec.ts` suffix and located in `tests/browser/`.
  ```
  tests/browser/counter.spec.ts
  ```

- **Running Tests:**  
  Use the provided `run.sh` script in `tests/browser/` to execute E2E tests.
  ```sh
  cd tests/browser
  ./run.sh
  ```

- **Example Test Structure:**
  ```typescript
  // counter.spec.ts
  import { test, expect } from '@playwright/test';

  test('counter increments', async ({ page }) => {
    await page.goto('/counter');
    await page.click('button#increment');
    await expect(page.locator('span#count')).toHaveText('1');
  });
  ```

## Commands

| Command           | Purpose                                                        |
|-------------------|----------------------------------------------------------------|
| /release-bump     | Prepare and bump the project version for a new release         |
| /refresh-artifacts| Refresh or update middleware/authz artifact files              |
| /new-fixture      | Add a new test fixture or scenario                            |
| /e2e-browser      | Add or update browser-based E2E tests                         |
| /update-docs      | Update documentation for new features, migrations, or releases |
| /update-ci        | Update CI configuration or dependency policy files             |
```
