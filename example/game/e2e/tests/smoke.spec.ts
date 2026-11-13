import { expect, test } from '@playwright/test';
import { createCharacter, enterWorld, hold, signUp, watchErrors } from './helpers';

test('a new player signs up, creates a mage and walks the town', async ({ page }) => {
  const errors = watchErrors(page);
  await signUp(page);
  const name = await createCharacter(page, 'Mage');
  await enterWorld(page);

  // The hero stands in town under its name label, with the hotbar below.
  await expect(page.getByText(name, { exact: true })).toBeVisible();
  await expect(page.locator('.ge-hotbar')).toBeVisible();

  // Moving keeps the session alive and the console clean.
  await hold(page, 'KeyD', 600);
  await hold(page, 'KeyA', 600);
  expect(errors).toEqual([]);
});

test('the skill tree opens with K and closes with Escape', async ({ page }) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  const tree = page.locator('.ge-tree');
  await page.keyboard.press('KeyK');
  await expect(tree).toBeVisible();
  // The mage tab lists its branches; a level 1 hero has no points yet.
  await expect(tree).toContainText('Fire Bolt');
  await page.keyboard.press('Escape');
  await expect(tree).toBeHidden();
  expect(errors).toEqual([]);
});
