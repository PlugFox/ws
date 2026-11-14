import { test, expect } from '@playwright/test';
import { signUp, createCharacter, enterWorld, watchErrors } from './helpers';

/**
 * Items: a new swordsman starts with the class kit (Sword, Cotton Shirt,
 * Guard worn), five Red Potions and 100 zeny; the windows show them with
 * their D&D tooltips.
 */
test('a new character carries the starting kit', async ({ page }) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  await page.keyboard.press('Alt+KeyE');
  const inv = page.locator('[data-win="inventory"]');
  await expect(inv).toContainText('100 Z');
  const potion = inv.locator('.u-slot', { has: page.locator('img[data-icon="item:red_potion"]') });
  await expect(potion).toContainText('5');
  await potion.hover();
  await expect(page.locator('.u-tip')).toContainText('Red Potion');
  await expect(page.locator('.u-tip')).toContainText('4d4+4');
  await page.keyboard.press('Alt+KeyQ');
  const eq = page.locator('[data-win="equip"]');
  for (const name of ['Sword', 'Cotton Shirt', 'Guard']) await expect(eq).toContainText(name);
  expect(errors).toEqual([]);
});

/**
 * The merchant cart (shop.ts, `shop_deal`): `?shop=` opens the Tool
 * Dealer from the spawn (a GAME_DEV server lets merchants trade from
 * anywhere). Sell the whole potion stack with Shift+click, type how many
 * to buy, and make both in one Deal: sales go first, so their zeny pays
 * for the buy. The Inventory window feeds the cart while the shop is open.
 */
test('the shop cart sells and buys in one deal', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, '/?shop=tool_dealer');
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  const shop = page.locator('[data-win="shop"]');
  await expect(shop).toContainText('Tool Dealer');
  const row = (name: string) => shop.locator('.list .row', { hasText: name });
  const cart = shop.locator('.cart .lines');

  // Sell: Shift+click puts the whole stack (5) in the cart.
  await shop.locator('.u-tabs button', { hasText: 'Sell' }).click();
  await row('Red Potion').click({ modifiers: ['Shift'] });
  await expect(cart.locator('.line', { hasText: 'Red Potion' }).locator('.u-field')).toHaveValue('5');

  // Buy: pick the row, type 4 in the bar, Enter adds them.
  await shop.locator('.u-tabs button', { hasText: 'Buy' }).click();
  await row('Red Potion').click();
  await shop.locator('.bar .u-field').fill('4');
  await shop.locator('.bar .u-field').press('Enter');
  await expect(cart.locator('h5')).toHaveText(['Buying', 'Selling']);
  const deal = shop.locator('.u-foot').getByRole('button', { name: 'Deal' });
  await expect(deal).toBeEnabled();
  const left = (await shop.locator('.sums b.left').innerText()).trim();
  await page.screenshot({ path: info.outputPath('cart.png') });
  await deal.click();
  await expect(shop.locator('.u-foot .u-note')).toContainText('Thank you');
  await expect(cart.locator('.line')).toHaveCount(0);

  // The bag: 4 potions bought after the 5 were sold, and the zeny the
  // cart promised.
  await page.keyboard.press('Alt+KeyE');
  const inv = page.locator('[data-win="inventory"]');
  await expect(inv).toContainText(left);
  const potion = inv.locator('.u-slot', { has: page.locator('img[data-icon="item:red_potion"]') });
  await expect(potion).toContainText('4');

  // With the shop open the inventory sells into the cart (double-click: the
  // stack), and shows what is left outside it.
  await potion.dblclick();
  await expect(cart.locator('.line', { hasText: 'Red Potion' }).locator('.u-field')).toHaveValue('4');
  await expect(potion).toContainText('0');
  await shop.locator('.u-foot').getByRole('button', { name: 'Clear' }).click();
  await expect(potion).toContainText('4');
  expect(errors).toEqual([]);
});
