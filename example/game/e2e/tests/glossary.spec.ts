import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, treeNode, watchErrors } from './helpers';

// The rules glossary (engine glossary.ts + kit/tooltip.ts, assets
// glossary.toml): rules words in tooltips and the guide link themselves;
// a term opens its own tip on hover, pins on a click (with ×), nests, and
// Esc closes the topmost first. Item tips lock after a short dwell so the
// pointer can reach their words.

async function centre(page: Page, selector: string, index = 0): Promise<{ x: number; y: number }> {
  const box = (await page.locator(selector).nth(index).boundingBox())!;
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

test('an item tip locks, its dice explain themselves, a click pins the chain', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  await page.keyboard.press('Alt+KeyE');
  const inv = page.locator('[data-win="inventory"]');
  const potion = inv.locator('.u-slot', { has: page.locator('img[data-icon="item:red_potion"]') });
  await potion.hover();
  const root = page.locator('.u-tip:not(.term)').first();
  await expect(root).toContainText('4d4+4');
  await expect(root.locator('.u-term[data-term="dice"]')).toHaveCount(1);
  // The dwell locks it (the slots under it stay reachable until then).
  await expect(root).toHaveClass(/\block\b/);
  // Into the tip, onto the dice: their own tip opens beside them.
  const from = await centre(page, '[data-win="inventory"] .u-slot:has(img[data-icon="item:red_potion"])');
  const to = await centre(page, '.u-tip .u-term[data-term="dice"]');
  await page.mouse.move(from.x, from.y);
  await page.mouse.move(to.x, to.y, { steps: 12 });
  const child = page.locator('.u-tip.term');
  await expect(child).toHaveCount(1);
  await expect(child).toContainText('8–20');
  await expect(child).toContainText('about 14');
  await page.screenshot({ path: info.outputPath('item-terms.png') });

  // A click pins the dice tip and the item tip under it (× on both).
  await page.locator('.u-tip .u-term[data-term="dice"]').first().click();
  await expect(page.locator('.u-tip.pin')).toHaveCount(2);
  await expect(page.locator('.u-tip.pin .u-tx-close button')).toHaveCount(2);
  // A term inside the pinned dice tip nests a third.
  await page.locator('.u-tip.term.pin .u-term[data-term="crit"]').click();
  await expect(page.locator('.u-tip.pin')).toHaveCount(3);
  await expect(page.locator('.u-tip.term[data-term="crit"]')).toContainText('natural 20');
  // The pointer leaving does not close pinned tips.
  await page.mouse.move(5, 5);
  await page.waitForTimeout(400);
  await expect(page.locator('.u-tip.pin')).toHaveCount(3);
  await page.screenshot({ path: info.outputPath('pinned-chain.png') });
  // Esc closes the topmost only; × closes a tip and what grew from it.
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-tip.pin')).toHaveCount(2);
  await page.locator('.u-tip.pin:not(.term) .u-tx-close button').click();
  await expect(page.locator('.u-tip.pin')).toHaveCount(0);
  // The inventory is still open: Esc went to the tips first.
  await expect(inv).toBeVisible();
  expect(errors).toEqual([]);
});

test('the guide explains the rules: terms pin and nest, the glossary page lists them', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, '/?guide=welcome');
  await createCharacter(page, 'Mage');
  await enterWorld(page);
  const win = page.locator('.u-guide');
  await win.locator('.gd-item', { hasText: 'How combat rolls work' }).click();
  await expect(win.locator('.gd-head h2')).toHaveText('How combat rolls work');

  // Hover: the tip opens after a moment, with the player's own numbers.
  await win.locator('.gd-page .u-term[data-term="dc"]').first().hover();
  const tip = page.locator('.u-tip.term');
  await expect(tip).toContainText('Save DC');
  await expect(tip).toContainText(/Your spell DC: \d+/);
  await page.mouse.move(5, 5);
  await expect(tip).toHaveCount(0);

  // Click: pinned, with ×; a term inside it nests a second; Esc, then ×.
  await win.locator('.gd-page .u-term[data-term="ac"]').first().click();
  const pinned = page.locator('.u-tip.pin');
  await expect(pinned).toHaveCount(1);
  await expect(pinned).toContainText('Armor Class');
  await expect(pinned.locator('.u-tx-close button')).toBeVisible();
  await pinned.locator('.u-term[data-term="attack_roll"]').first().click();
  await expect(page.locator('.u-tip.pin')).toHaveCount(2);
  await page.screenshot({ path: info.outputPath('guide-chain.png') });
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-tip.pin')).toHaveCount(1);
  await expect(win).toBeVisible();
  // "Open in guide": the glossary page, that term open.
  await page.locator('.u-tip.pin .u-tlink').click();
  await expect(win.locator('.gd-head h2')).toHaveText('Rules and glossary');
  await expect(win.locator('.gd-term.on[data-id="ac"]')).toBeVisible();
  await page.locator('.u-tip.pin .u-tx-close button').click();
  await expect(page.locator('.u-tip.pin')).toHaveCount(0);

  // The page: groups, a search, rows that open in place.
  await expect(win.locator('.gd-gloss h3').first()).toBeVisible();
  await win.locator('.gd-search').fill('burning');
  await expect(win.locator('.gd-term[data-id="burning"]')).toBeVisible();
  await expect(win.locator('.gd-term[data-id="ac"]')).toHaveCount(0);
  await win.locator('.gd-term[data-id="burning"] .gd-trow').click();
  await expect(win.locator('.gd-term.on')).toContainText('Body save');
  await win.locator('.gd-search').fill('');
  await win.locator('.gd-trow', { hasText: 'Level multiplier' }).click();
  await expect(win.locator('.gd-term.on[data-id="k"]')).toContainText(/Now k = \d/);
  await page.screenshot({ path: info.outputPath('guide-rules.png') });

  // A skill tip: its dice, damage type, save and condition link; ×k too.
  await win.locator('.u-sys .close').click();
  await page.keyboard.press('KeyK');
  await treeNode(page, 'Cold Bolt').hover();
  const skill = page.locator('.u-tip:not(.term)').first();
  await expect(skill).toContainText('Cold Bolt');
  await expect(skill.locator('.u-term[data-term="save_body"]')).toHaveCount(1);
  await expect(skill.locator('.u-term[data-term="k"]')).toHaveCount(1);
  await page.screenshot({ path: info.outputPath('skill-tip.png') });
  expect(errors).toEqual([]);
});

test.describe('touch', () => {
  test.use({ hasTouch: true, viewport: { width: 390, height: 740 } });

  test('a tap pins a term as a bottom sheet with a way back', async ({ page }, info) => {
    const errors = watchErrors(page);
    await signUp(page, '/?guide=welcome');
    await createCharacter(page, 'Mage');
    await enterWorld(page);
    const win = page.locator('.u-guide');
    await win.getByRole('button', { name: '‹ Topics' }).tap();
    await win.locator('.gd-item', { hasText: 'How combat rolls work' }).tap();
    await win.locator('.gd-page .u-term[data-term="ac"]').first().tap();
    const sheet = page.locator('.u-tip.sheet:not(.under)');
    await expect(sheet).toHaveCount(1);
    await expect(sheet).toContainText('Armor Class');
    await sheet.locator('.u-term[data-term="attack_roll"]').first().tap();
    await expect(sheet).toContainText('Attack roll');
    await expect(sheet.locator('.u-crumb')).toContainText('Armor Class › Attack roll');
    await page.screenshot({ path: info.outputPath('sheet.png') });
    await sheet.locator('.u-crumb button').tap();
    await expect(sheet).toContainText('Armor Class');
    await expect(page.locator('.u-tip.sheet')).toHaveCount(1);
    // A tap outside closes the top one.
    await win.locator('.gd-head h2').tap();
    await expect(page.locator('.u-tip.sheet')).toHaveCount(0);
    expect(errors).toEqual([]);
  });
});
