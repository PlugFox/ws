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
  await expect(page.locator('.u-tip')).toContainText('2d4+2');
  await page.keyboard.press('Alt+KeyQ');
  const eq = page.locator('[data-win="equip"]');
  for (const name of ['Sword', 'Cotton Shirt', 'Guard']) await expect(eq).toContainText(name);
  expect(errors).toEqual([]);
});
