import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

/** The Social window on its Party tab (opened when it is not showing). */
async function partyTab(page: Page) {
  const social = page.locator('[data-win="social"]');
  const tab = social.locator('.u-tabs button.on');
  // Right after entering the world the key may land before the HUD listens:
  // press again until the window shows.
  await expect(async () => {
    if (!(await social.isVisible().catch(() => false))) await page.keyboard.press('Alt+KeyZ');
    await expect(tab).toHaveText(/^Party/, { timeout: 2_000 });
  }).toPass({ timeout: 20_000 });
  return social;
}

// Parties belong to accounts (lobby.rs): no ready check, and the party
// outlives the mission, the character select and a sign-out. `?mission=`
// (GAME_DEV) opens a private party of one on the first town join.
/** Esc closes the open windows one by one, then opens the game menu. */
async function openMenu(page: Page) {
  const menu = page.locator('.u-menu');
  for (let i = 0; i < 5 && !(await menu.isVisible()); i++) {
    await page.keyboard.press('Escape');
    await page.waitForTimeout(150);
  }
  await expect(menu).toBeVisible();
}

test('the party outlives a mission, the character select and a sign-out', async ({ page }, info) => {
  const errors = watchErrors(page);
  const user = await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);

  let social = await partyTab(page);
  await expect(social).toContainText('Party · Poring Island Survival');
  const code = (await social.textContent())!.match(/code ([A-Z0-9]{5})/)![1];
  // The leader starts at once: there is no Ready button any more.
  await expect(social.getByRole('button', { name: /Ready/ })).toHaveCount(0);
  await expect(social).toContainText('leader');
  await social.getByRole('button', { name: 'Start', exact: true }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);

  // End the run (GAME_DEV `/end`); everyone is sent back to town.
  await page.keyboard.press('Enter');
  const say = page.locator('.u-chat input');
  await expect(say).toBeVisible();
  await say.fill('/end lose');
  await say.press('Enter');
  const results = page.locator('.u-results');
  await expect(results).toBeVisible();
  await expect(page.locator('body')).not.toContainText(/· Stage-\d+ ·/, { timeout: 30_000 });
  await results.getByRole('button', { name: 'Close', exact: true }).click();

  // Back in town: the same party, ready to go again.
  social = await partyTab(page);
  await expect(social).toContainText(`code ${code}`);
  await expect(social.getByRole('button', { name: 'Start', exact: true })).toBeEnabled();
  await page.screenshot({ path: info.outputPath('after-mission.png') });

  // The character select and back: still the party.
  await openMenu(page);
  await page.locator('.u-menu button', { hasText: 'Character select' }).click();
  await enterWorld(page);
  social = await partyTab(page);
  await expect(social).toContainText(`code ${code}`);

  // Log out and in again: still the party.
  await openMenu(page);
  await page.locator('.u-menu button', { hasText: 'Log out' }).click();
  await page.getByPlaceholder('username').fill(user);
  await page.getByPlaceholder('password').fill('e2e-password');
  await page.getByRole('button', { name: 'Log in' }).click();
  await enterWorld(page);
  social = await partyTab(page);
  await expect(social).toContainText(`code ${code}`);
  await expect(social).toContainText('1/8');
  expect(errors).toEqual([]);
});

// The character select has its own Log out: back to the sign-in, with the
// session revoked on the server.
test('Log out on the character select revokes the session', async ({ page }) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Mage');
  const logout = page.waitForResponse((r) => r.url().endsWith('/api/auth/logout'));
  await page.getByRole('button', { name: 'Log out', exact: true }).click();
  expect((await logout).status()).toBe(200);
  await expect(page.getByRole('button', { name: 'Log in' })).toBeVisible();
  expect(errors).toEqual([]);
  // Nothing is left to renew: the characters need a new sign-in (the 401
  // this probe draws is logged by the browser, so it comes after the check).
  expect(await page.evaluate(() => fetch('/api/characters').then((r) => r.status))).toBe(401);
});
