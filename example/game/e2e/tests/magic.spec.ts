import { expect, test } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

// Magic items (server combat/magic.rs): `/give <item>` is the GAME_DEV hook
// that drops one into the speaker's bag. Their names are green (uncommon)
// or blue (rare), the tooltip lists the enchantment, and wearing one puts
// it in the equipment window and the weapon in the hero's hand.
test('magic items: rarity colours, enchantment lines, worn in hand', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);

  const say = async (text: string) => {
    await page.keyboard.press('Enter');
    const input = page.locator('.u-chat input');
    await expect(input).toBeVisible();
    await input.fill(text);
    await input.press('Enter');
  };
  for (const id of ['ember_blade', 'striding_boots', 'protection_ring']) await say(`/give ${id}`);

  await page.keyboard.press('Alt+KeyE');
  const inv = page.locator('[data-win="inventory"]');
  const slot = (id: string) => inv.locator('.u-slot', { has: page.locator(`img[data-icon="item:${id}"]`) });
  const tabs = inv.locator('.u-tabs button');
  if (await tabs.count()) await tabs.filter({ hasText: /Equip/ }).first().click();
  await expect(slot('ember_blade')).toBeVisible();

  const tip = page.locator('.u-tip');
  await slot('ember_blade').hover();
  await expect(tip.locator('.t.r-uncommon')).toHaveText('Ember Blade');
  await expect(tip).toContainText('+1d4 damage on every weapon hit');
  await expect(tip).toContainText('Uncommon magic item');
  await page.screenshot({ path: info.outputPath('tooltip.png') });
  await slot('protection_ring').hover();
  await expect(tip.locator('.t.r-rare')).toHaveText('Ring of Protection');
  await expect(tip).toContainText('+1 to every save');

  // Worn: the equipment window names it in green; the boots too.
  await slot('ember_blade').dblclick();
  await slot('striding_boots').dblclick();
  await page.keyboard.press('Alt+KeyQ');
  const eq = page.locator('[data-win="equip"]');
  await expect(eq.locator('.r-uncommon', { hasText: 'Ember Blade' }).first()).toBeVisible();
  await expect(eq.locator('.r-uncommon', { hasText: 'Boots of Striding' }).first()).toBeVisible();
  await page.screenshot({ path: info.outputPath('equipped.png') });
  expect(errors).toEqual([]);

  // The character select draws the hero with its weapon (the swing frames
  // in the blade's own colours).
  const select = page.locator('[data-win="dialog"]').getByText('Character select');
  await expect(async () => {
    if (!(await select.isVisible())) await page.keyboard.press('Escape');
    await expect(select).toBeVisible({ timeout: 500 });
  }).toPass();
  await select.click();
  await expect(page.getByRole('button', { name: /Enter world/ })).toBeVisible();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: info.outputPath('select.png') });
});
