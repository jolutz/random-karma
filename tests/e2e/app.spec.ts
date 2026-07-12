import { expect, test, type Page } from '@playwright/test';

async function openSettings(page: Page) {
  const settings = page.getByRole('button', { name: 'Settings' });
  if ((await settings.getAttribute('aria-expanded')) !== 'true') {
    await settings.click({ force: true });
  }
}

async function disablePrecaching(page: Page) {
  await openSettings(page);
  const checkbox = page.getByRole('checkbox', { name: 'Enable Pre-caching' });
  if (await checkbox.isChecked()) {
    await checkbox.uncheck();
  }
}

test('loads the application with its default controls', async ({ page }) => {
  await page.goto('./');

  await expect(page).toHaveTitle('Random Karma · Selection Calculator');
  await expect(page.getByRole('heading', { name: 'Random Karma Configuration' })).toBeVisible();
  await expect(page.getByLabel('Lap Count:')).toHaveValue('25');
  await expect(page.getByLabel('Player Count:')).toHaveValue('32');
  await expect(page.getByLabel('Target Time:')).toBeVisible();
  await expect(page.locator('.settings-summary')).toContainText(/Cache: \d+\/100/);
  await openSettings(page);
  await expect(page.getByRole('radio', { name: /Bounded/ })).toBeChecked();
  await expect(page.getByRole('radio', { name: /Legacy/ })).not.toBeChecked();
});

test('applies parameter changes and reaches a completed calculation state', async ({ page }) => {
  await page.goto('./');
  await disablePrecaching(page);

  await page.getByLabel('Lap Count:').fill('3');
  await page.getByLabel('Lap Count:').press('Enter');
  await page.getByLabel('Player Count:').fill('2');
  await page.getByLabel('Player Count:').press('Enter');

  await expect(page.getByLabel('Lap Count:')).toHaveValue('3');
  await expect(page.getByLabel('Player Count:')).toHaveValue('2');
  await expect(page.locator('.results-count')).toHaveText('2 selections', { timeout: 15_000 });
  await expect(page.locator('.loading-indicator')).toBeHidden();
  await expect(page.locator('.big-car-table th')).toContainText(['Set #', 'Total Time', '% Off Target', 'Car 1', 'Car 2', 'Car 3']);
});

test('opens settings, changes a solver setting, and clears the cache', async ({ page }) => {
  await page.goto('./');
  await openSettings(page);

  const timeout = page.getByLabel('Calculation Timeout (seconds):');
  await timeout.fill('1');
  await timeout.press('Enter');
  await expect(timeout).toHaveValue('1');

  await page.getByRole('button', { name: 'Clear Cache' }).click();
  await expect(page.locator('.cache-status-global')).toHaveText('Total entries: 0');
});

test('switches solver strategy and recalculates with separate state', async ({ page }) => {
  await page.goto('./');
  await disablePrecaching(page);

  const bounded = page.getByRole('radio', { name: /Bounded/ });
  const legacy = page.getByRole('radio', { name: /Legacy/ });
  await expect(bounded).toBeChecked();
  await legacy.check();
  await expect(legacy).toBeChecked();
  await expect(bounded).not.toBeChecked();
  await expect(page.locator('.loading-indicator')).toBeHidden({ timeout: 15_000 });
});

test('replaces the dataset from mocked clipboard text', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.addInitScript(() => {
    const clipboard = {
      readText: async () => 'E2E-ONE,1:00\nE2E-TWO,1:01\nE2E-THREE,1:02',
      writeText: async () => undefined,
    };
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: clipboard,
    });
  });
  await page.goto('./');
  await openSettings(page);

  await page.getByRole('button', { name: 'Paste Car Data from Clipboard' }).click();
  await expect(page.locator('.clipboard-feedback')).toHaveText(
    'Successfully loaded 3 cars from clipboard.',
  );
  await expect(page.locator('.slider-info')).toHaveText('Max: 3');
});
