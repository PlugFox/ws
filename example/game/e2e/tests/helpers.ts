import { expect, type Page } from '@playwright/test';

/** Collects uncaught exceptions and console errors of a page. */
export function watchErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(`console: ${m.text()}`);
  });
  return errors;
}

/** A name unique per run (accounts and characters are global). */
export function unique(prefix: string): string {
  return `${prefix}${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

/** Registers a fresh account through the login screen. */
export async function signUp(page: Page): Promise<string> {
  const user = unique('e2e_');
  await page.goto('/');
  await page.getByPlaceholder('username').fill(user);
  await page.getByPlaceholder('password').fill('e2e-password');
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.locator('.login')).toHaveCount(0);
  return user;
}

/** Creates a character of `className` (as the class button shows it). */
export async function createCharacter(page: Page, className = 'Mage'): Promise<string> {
  const name = unique('Hero');
  const newChar = page.getByRole('button', { name: /New character/ }).first();
  // A fresh account opens straight on the creation screen.
  if (await newChar.isVisible().catch(() => false)) await newChar.click();
  await page.locator('.ge-field input').fill(name);
  await page.locator('.ge-classes button', { hasText: className }).first().click();
  await page.getByRole('button', { name: /^Create/ }).click();
  await expect(page.getByRole('button', { name: /Enter world/ })).toBeVisible();
  return name;
}

/** Enters the world with the selected character and waits for the town
 * (the debug line names the hub channel once the map is in). */
export async function enterWorld(page: Page): Promise<void> {
  await page.getByRole('button', { name: /Enter world/ }).click();
  await expect(page.locator('body')).toContainText(/· Hub-\d+ ·/);
}

/** Holds a key (by `KeyboardEvent.code` name, e.g. `KeyD`) for `ms`. */
export async function hold(page: Page, key: string, ms: number): Promise<void> {
  await page.keyboard.down(key);
  await page.waitForTimeout(ms);
  await page.keyboard.up(key);
}
