import { expect, test, type Page } from '@playwright/test';
import { cooldown, createCharacter, enterWorld, hotkey, learn, signUp, slot, treeNode, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

async function say(page: Page, text: string): Promise<void> {
  await page.keyboard.press('Enter');
  const input = page.locator('.u-chat input');
  await expect(input).toBeVisible();
  await input.fill(text);
  await input.press('Enter');
}

/** Drags `from` onto `to` with the mouse (kit/drag: a press that moves). */
async function drag(page: Page, from: ReturnType<Page['locator']>, to: ReturnType<Page['locator']>): Promise<void> {
  const a = (await from.boundingBox())!;
  const b = (await to.boundingBox())!;
  await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
  await page.mouse.down();
  await page.mouse.move(a.x + a.width / 2 + 10, a.y + a.height / 2 + 10, { steps: 3 });
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 8 });
  await page.mouse.up();
}

// Hotbar chains and the AI (server `hub/cast.rs` chains and AI pass,
// engine skillui.ts): Fire Bolt and Cold Bolt chained on slot 1 cast in
// turn, one per press; Lightning Bolt marked Auto in the tree is cast by the
// AI (its slot 2 shows the cooldown) only while the AI is on.
test('a chained slot casts its skills in turn; the AI casts auto skills off the bar', async ({ page }, info) => {
  test.setTimeout(120_000);
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  await page.keyboard.press('KeyK');
  await learn(page, 'Fire Bolt', 1);
  await learn(page, 'Cold Bolt', 1);
  await learn(page, 'Lightning Bolt', 1);
  await slot(page, 'Fire Bolt', 0);
  // Cold Bolt onto slot 1's + cell: a chain of two.
  const first = page.locator('.u-hotbar .u-hk').nth(0);
  await drag(page, treeNode(page, 'Cold Bolt').locator('.u-slot'), first.locator('.fan .fc.add'));
  await expect(first).toHaveClass(/\bchain\b/);
  await expect(first.locator('.chainmark')).toHaveText('×2');
  await expect(first.locator('.fan .fc[data-skill]')).toHaveCount(2);
  // Lightning Bolt: Auto from its badge (slot 2 only to watch its cooldown).
  await slot(page, 'Lightning Bolt', 1);
  await treeNode(page, 'Lightning Bolt').locator('.ab').click();
  await expect(treeNode(page, 'Lightning Bolt').locator('.ab')).toHaveClass(/\bon\b/);
  await expect(page.locator('.u-skills .autos .ac')).toHaveCount(1);
  await info.attach('setup', { body: await page.screenshot(), contentType: 'image/png' });
  await page.locator('.u-skills').getByRole('button', { name: 'Close', exact: true }).click();
  await expect(page.locator('.u-skills')).toBeHidden();

  // The AI off first (the backquote key): nothing casts by itself.
  await page.keyboard.press('Backquote');
  await expect(page.locator('.u-hotbar .u-ai')).toHaveClass(/\boff\b/);

  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/, { timeout: 60_000 });
  await page.waitForTimeout(2500);
  // Targets close by that last a while.
  for (let i = 0; i < 3; i++) await say(page, '/spawn poring');
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.waitForTimeout(800);

  // Press 1: Fire Bolt, then the slot shows Cold Bolt up next; press again.
  await expect(first).toHaveAttribute('data-name', 'Fire Bolt');
  await page.keyboard.press('Digit1');
  await expect(first).toHaveAttribute('data-name', 'Cold Bolt', { timeout: 5000 });
  await page.waitForTimeout(400);
  await page.keyboard.press('Digit1');
  await expect(first).toHaveAttribute('data-name', 'Fire Bolt', { timeout: 5000 });
  await info.attach('chain', { body: await page.screenshot(), contentType: 'image/png' });

  // AI off: Lightning Bolt waits (marked, never cast).
  await expect(hotkey(page, 'Lightning Bolt')).toHaveClass(/\bauto\b/);
  expect(await cooldown(page, 'Lightning Bolt')).toBe(0);
  // The AI on (its button): Lightning Bolt gets cast by itself.
  await page.locator('.u-hotbar .u-ai').click();
  await expect(page.locator('.u-hotbar .u-ai')).not.toHaveClass(/\boff\b/);
  await expect.poll(() => cooldown(page, 'Lightning Bolt'), { timeout: 10_000 }).toBeGreaterThan(0);
  await info.attach('ai', { body: await page.screenshot(), contentType: 'image/png' });
  expect(errors).toEqual([]);
});
