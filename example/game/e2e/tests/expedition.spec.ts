import { expect, test } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

// The expedition window (missions.ts): the Kafra's "Choose an expedition"
// lists the ladder (`MISSION_LIST`) beside the selected mission's page.
// `?shop=kafra` opens the Kafra from the spawn (a GAME_DEV server lets
// mission givers answer from anywhere). Which of the later missions are
// still "coming soon" depends on the baked pack, so only Poring Island is
// assumed open.
test('the Kafra opens the expedition window: list, page, keys, start', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, '/?shop=kafra');
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  const dialog = page.locator('[data-win="dialog"]');
  await dialog.getByText('Choose an expedition').click();
  const win = page.locator('.u-missions');
  await expect(win).toBeVisible();
  const items = win.locator('.ex-item');
  await expect(items).toHaveCount(4);
  // The window opens on the mission that suits the hero (e2e heroes start at
  // GAME_DEV_LEVEL, so which one depends on the server's setting).
  const chosen = win.locator('.ex-item.on');
  await expect(chosen).toHaveCount(1);
  // (A level on a band edge suits two missions.)
  expect(await win.locator('.ex-item .ex-st', { hasText: 'Recommended' }).count()).toBeGreaterThan(0);
  const name = (await chosen.locator('.tx b').textContent())!.trim();
  await expect(win.locator('.mi-hero h2')).toHaveText(name);
  const poring = win.locator('.ex-item[data-id="poring_island_survival"]');
  await poring.click();
  await expect(poring).toHaveClass(/\bon\b/);
  await expect(win.locator('.mi-hero h2')).toHaveText('Poring Island Survival');
  await expect(win.locator('.ex-boss')).toContainText('Angeling');
  await page.screenshot({ path: info.outputPath('window.png') });

  // A click selects another mission; the arrow keys walk the list.
  await items.nth(1).click();
  await expect(items.nth(1)).toHaveClass(/\bon\b/);
  await expect(win.locator('.mi-hero h2')).not.toHaveText('Poring Island Survival');
  await page.keyboard.press('ArrowUp');
  await expect(poring).toHaveClass(/\bon\b/);

  await win.locator('.u-foot').getByRole('button', { name: 'Start', exact: true }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  expect(errors).toEqual([]);
});
