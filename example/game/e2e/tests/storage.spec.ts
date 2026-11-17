import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, unique, watchErrors } from './helpers';

async function openMenu(page: Page) {
  const menu = page.locator('.u-menu');
  for (let i = 0; i < 5 && !(await menu.isVisible()); i++) {
    await page.keyboard.press('Escape');
    await page.waitForTimeout(150);
  }
  await expect(menu).toBeVisible();
}

// The account storage (server `session/storage`, `db/storage_repo.rs`,
// engine storage.ts): a mage deposits potions and zeny in town, a second
// character of the same account takes them out. Every move is one
// database transaction with the bag, so a reload shows the same.
test('the account storage moves items and zeny between characters', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  const win = page.locator('[data-win="storage"]');
  const bag = (key: string) => win.locator(`.pane.bag .u-slot[data-key="${key}"]`);
  const store = (key: string) => win.locator(`.pane.store .u-slot[data-key="${key}"]`);
  await page.keyboard.press('Alt+KeyB');
  await expect(win).toBeVisible();
  await expect(bag('red_potion')).toContainText('5');

  // Double-click: the whole stack goes in.
  await bag('red_potion').dblclick();
  await expect(store('red_potion')).toContainText('5');
  await expect(bag('red_potion')).toHaveCount(0);

  // Zeny: the whole bag (a new character's 100).
  const zeny = win.locator('.zeny');
  await zeny.getByRole('button', { name: 'All', exact: true }).click();
  await zeny.getByRole('button', { name: 'Deposit', exact: true }).click();
  await expect(win.locator('.zeny')).toContainText('Storage 100 Z');

  // Search and tabs: the potion is a consumable, not gear.
  await win.locator('input[type="search"], .top input').first().fill('red');
  await expect(store('red_potion')).toBeVisible();
  await win.locator('.top input').first().fill('');
  await info.attach('storage', { body: await page.screenshot(), contentType: 'image/png' });

  // A second character of the account finds it all.
  await openMenu(page);
  await page.locator('.u-menu button', { hasText: 'Character select' }).click();
  const newChar = page.getByRole('button', { name: /New character/ }).first();
  await expect(newChar).toBeVisible({ timeout: 30_000 });
  await newChar.click();
  await page.locator('.ge-field input').fill(unique('Alt'));
  await page.locator('.ge-classes button', { hasText: 'Acolyte' }).first().click();
  await page.getByRole('button', { name: /^Create/ }).click();
  await enterWorld(page);
  await expect(async () => {
    if (!(await win.isVisible())) await page.keyboard.press('Alt+KeyB');
    await expect(store('red_potion')).toContainText('5', { timeout: 2000 });
  }).toPass({ timeout: 20_000 });
  await store('red_potion').dblclick();
  await expect(bag('red_potion')).toContainText('10');
  await zeny.locator('button.bal', { hasText: 'Storage' }).click();
  await zeny.getByRole('button', { name: 'Withdraw', exact: true }).click();
  await expect(win.locator('.zeny')).toContainText('Bag 200 Z');
  expect(errors).toEqual([]);
});
