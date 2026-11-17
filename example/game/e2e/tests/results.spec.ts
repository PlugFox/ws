import { expect, test } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

// The result screen after a mission: `/end win` is the GAME_DEV test hook
// that ends the running mission at once (hub.rs `chat`).
test('a finished mission shows the result screen, back in town too', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);

  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  // A few seconds on the clock (the wave may or may not have arrived).
  await page.waitForTimeout(3000);

  await page.keyboard.press('Enter');
  const say = page.locator('.u-chat input');
  await expect(say).toBeVisible();
  await say.fill('/end win');
  await say.press('Enter');

  const results = page.locator('.u-results');
  await expect(results).toBeVisible();
  await expect(results.locator('.banner')).toHaveText('Victory!');
  await expect(results.locator('tbody tr.me')).toHaveCount(1);
  await expect(results).toContainText(/Back to town in \d+ s/);
  await page.screenshot({ path: info.outputPath('results.png') });

  for (const tab of ['Damage', 'Loot']) {
    await results.locator('.u-tabs button', { hasText: tab }).click();
    await expect(results.locator('.u-tabs button.on')).toHaveText(tab);
  }
  await expect(results).toContainText('Base experience');

  // The party is sent home; the screen stays until closed.
  await expect(page.locator('body')).not.toContainText(/· Stage-\d+ ·/, { timeout: 30_000 });
  await expect(results).toBeVisible();
  await results.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(results).toBeHidden();
  expect(errors).toEqual([]);
});
