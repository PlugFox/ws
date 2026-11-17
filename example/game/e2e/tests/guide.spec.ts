import { expect, test } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

// The Adventurer's Guide (guide.ts, client guide.dart): a new character's
// first arrival opens it (`?guide=welcome` stands in for level 1: e2e heroes
// start higher), it minimises and closes, and the Kafra opens it again
// (`?shop=kafra` opens the Kafra from the spawn on a GAME_DEV server).
test('the guide greets a new character, minimises, closes and reopens from the menu', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, '/?guide=welcome');
  await createCharacter(page, 'Acolyte');
  await enterWorld(page);

  const win = page.locator('.u-guide');
  await expect(win).toBeVisible();
  await expect(win.locator('.gd-welcome')).toBeVisible();
  await expect(win.locator('.gd-head h2')).toHaveText('Getting started');
  const topics = win.locator('.gd-item');
  expect(await topics.count()).toBeGreaterThanOrEqual(10);
  await page.screenshot({ path: info.outputPath('welcome.png') });

  // Topics: a click opens one; Next walks on; the keys follow the bindings.
  await topics.filter({ hasText: 'Controls' }).click();
  await expect(win.locator('.gd-head h2')).toHaveText('Controls');
  await expect(win.locator('.gd-keys')).toContainText('skill tree');
  await win.getByRole('button', { name: 'Next' }).click();
  await expect(win.locator('.gd-head h2')).toHaveText('Izlude');
  await topics.filter({ hasText: 'Expeditions' }).click();
  await expect(win.locator('.gd-page')).toContainText('Poring Island');
  await expect(win.locator('.gd-fig svg').first()).toBeVisible();
  await topics.filter({ hasText: 'Skills and auto-cast' }).click();
  await page.screenshot({ path: info.outputPath('skills.png') });

  // Minimise keeps the title bar; a second press brings it back.
  await win.locator('.u-sys .mini').click();
  await expect(win).toHaveClass(/\bmin\b/);
  await expect(win.locator('.gd-page')).toBeHidden();
  await win.locator('.u-sys .mini').click();
  await expect(win.locator('.gd-page')).toBeVisible();
  await win.locator('.u-sys .close').click();
  await expect(win).toHaveCount(0);

  // The game menu has it too.
  await page.keyboard.press('Escape');
  await page.locator('[data-win="dialog"]').getByText("Adventurer's Guide").click();
  await expect(win).toBeVisible();
  await expect(win.locator('.gd-welcome')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('the Kafra opens the guide; a hero past level 1 is not greeted', async ({ page }) => {
  const errors = watchErrors(page);
  await signUp(page, '/?shop=kafra');
  await createCharacter(page, 'Thief');
  await enterWorld(page);
  const win = page.locator('.u-guide');
  const dialog = page.locator('[data-win="dialog"]');
  await expect(dialog.getByText('Choose an expedition')).toBeVisible();
  await expect(win).toHaveCount(0);
  await dialog.getByText("Adventurer's Guide").click();
  await expect(win).toBeVisible();
  await expect(win.locator('.gd-welcome')).toHaveCount(0);
  // The Kafra sent the ladder: the expeditions page lists it.
  await win.locator('.gd-item', { hasText: 'Expeditions' }).click();
  await expect(win.locator('.gd-page')).toContainText('Prontera Culvert');
  expect(errors).toEqual([]);
});

test('the guide on a phone: the topics, then a page with a way back', async ({ page }) => {
  const errors = watchErrors(page);
  await page.setViewportSize({ width: 390, height: 740 });
  await signUp(page, '/?guide=welcome');
  await createCharacter(page, 'Mage');
  await enterWorld(page);
  const win = page.locator('.u-guide');
  // A welcome opens on its first page; Topics goes back to the list.
  await expect(win.locator('.gd-page')).toBeVisible();
  await expect(win.locator('.gd-list')).toHaveCount(0);
  await win.getByRole('button', { name: '‹ Topics' }).click();
  await expect(win.locator('.gd-list')).toBeVisible();
  await win.locator('.gd-item', { hasText: 'Parties' }).click();
  await expect(win.locator('.gd-head h2')).toHaveText('Parties');
  expect(errors).toEqual([]);
});
