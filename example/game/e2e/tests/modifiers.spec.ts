import { expect, test } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

test('the leader sets mission modifiers and the run carries them', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);

  // `?mission=` opened a lobby of one: its Modifiers button opens the
  // mission window on the party's mission, editable by the leader.
  await page.getByRole('button', { name: 'Modifiers…' }).click();
  const win = page.locator('.u-missions');
  await expect(win.locator('.mo-box')).toBeVisible();
  await win.locator('.mo-c', { hasText: 'Leaden Feet' }).click();
  await win.locator('.mo-c', { hasText: 'Darkness' }).click();
  await win.locator('.mo-tier .u-range').fill('3');
  // The server's echo: risk 3×4 + 6 + 8, each kill's loot share shown
  // (tier 3: ×1.12 monsters, each worth 1.12^-0.6 ≈ 93 %).
  await expect(win.locator('.mo-sum .risk')).toHaveText('26');
  await expect(win.locator('.mo-sum')).toContainText('each kill 93% loot');
  await expect(win.locator('.mo-c.soon').first()).toBeVisible();
  await page.screenshot({ path: info.outputPath('window.png') });
  await win.getByRole('button', { name: 'Close', exact: true }).click();

  // The party panel sums it up.
  await expect(page.locator('body')).toContainText('Modifiers: Hard III · Leaden Feet · Darkness · risk 26');

  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  // The tier and both curses by the timeline.
  await expect(page.locator('.ge-tl .mods i')).toHaveCount(3);
  await expect(page.locator('.ge-tl .mods i.tier')).toHaveText('T3');
  await page.screenshot({ path: info.outputPath('hud.png') });
  expect(errors).toEqual([]);
});
