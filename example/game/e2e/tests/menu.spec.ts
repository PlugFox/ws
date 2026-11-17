import { test, expect } from '@playwright/test';
import { signUp, createCharacter, enterWorld, watchErrors } from './helpers';
/** The game menu (Esc) and the debug overlay (F1). */
test('Esc menu: character select and back, log out; F1 cycles the debug overlay', async ({ page }) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Mage');
  await enterWorld(page);
  await page.keyboard.press('F1');
  await expect(page.locator('.u-dbg-mini')).toBeVisible();
  await page.keyboard.press('F1');
  await expect(page.locator('.u-dbg')).toBeVisible();
  for (let i = 0; i < 5; i++) await page.keyboard.press('F1');
  await expect(page.locator('.u-dbg')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-win="dialog"]')).toContainText('Character select');
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-win="dialog"]')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await page.locator('.u-menu button', { hasText: 'Character select' }).click();
  await expect(page.getByRole('button', { name: /Enter world/ })).toBeVisible();
  await enterWorld(page);
  await page.waitForTimeout(1000);
  await page.keyboard.press('Escape');
  await page.locator('.u-menu button', { hasText: 'Log out' }).click();
  await expect(page.getByPlaceholder('password')).toBeVisible();
  expect(errors).toEqual([]);
});
