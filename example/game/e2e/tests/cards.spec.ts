import { expect, test } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

// Monster cards (server combat/cards.rs, engine cards.ts): `/give` drops two
// weapon cards into the bag; the Card Sockets window (Equipment → Cards)
// puts them into the worn sword, which takes their words ("Stinging Sword
// of Kindling [3]", [n] = its sockets); a click takes one out again. The Card tab of the
// inventory opens a card large, and it tilts under the pointer.
test('cards go into the sword, rename it and come out again', async ({ page }, info) => {
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
  await say('/give drops_card');
  await say('/give hornet_card');

  await page.keyboard.press('Alt+KeyQ');
  const eq = page.locator('[data-win="equip"]');
  await eq.getByRole('button', { name: 'Cards' }).click();
  const win = page.locator('[data-win="sockets"]');
  await expect(win).toBeVisible();
  // The worn sword (3 sockets, Cotton Shirt and Guard are hosts too).
  await win.locator('.hosts .u-slot', { has: page.locator('img[data-icon="item:sword"]') }).click();
  await expect(win.locator('.title')).toHaveText('Sword [3]');
  await expect(win.locator('.sock')).toHaveCount(3);

  await win.locator('.pool button', { hasText: 'Drops Card' }).click();
  await expect(win.locator('.title')).toHaveText('Sword of Kindling [3]');
  await win.locator('.pool button', { hasText: 'Hornet Card' }).click();
  await expect(win.locator('.title')).toHaveText('Stinging Sword of Kindling [3]');
  await expect(eq).toContainText('Stinging Sword of Kindling [3]');
  await page.screenshot({ path: info.outputPath('socketed.png') });

  // The tooltip lists the cards and what they do, and the free socket.
  await eq.locator('img[data-icon="item:sword"]').first().hover();
  const tip = page.locator('.u-tip');
  await expect(tip).toContainText('Drops Card');
  await expect(tip).toContainText('sets the monster Burning');
  await expect(tip).toContainText('empty socket');

  // A click on a socketed card takes it out.
  await win.locator('.sock.full').first().click();
  await expect(win.locator('.title')).toHaveText('Stinging Sword [3]');

  // While the sockets are open the inventory feeds them (a double-click
  // sockets a card): close them to look at one.
  await win.locator('button.close').click();
  // The card itself, large: it tilts toward the pointer.
  await page.keyboard.press('Alt+KeyE');
  const inv = page.locator('[data-win="inventory"]');
  await inv.locator('.u-tabs button', { hasText: 'Card' }).click();
  await inv.locator('.u-slot', { has: page.locator('img[data-icon="item:drops_card"]') }).dblclick();
  const card = page.locator('[data-win="card"] .gc-card');
  await expect(card).toBeVisible();
  await expect(card).toContainText('Drops');
  const box = (await card.boundingBox())!;
  await page.mouse.move(box.x + box.width * 0.85, box.y + box.height * 0.2);
  await expect.poll(async () => card.evaluate((e) => (e as HTMLElement).style.getPropertyValue('--ry'))).not.toBe('');
  await page.screenshot({ path: info.outputPath('card.png') });
  expect(errors).toEqual([]);
});
