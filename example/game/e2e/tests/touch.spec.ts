import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, learn, signUp, slot, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

test.use({ hasTouch: true });

/** Raw touch input through the DevTools protocol (Playwright only taps). */
async function touch(page: Page) {
  const cdp = await page.context().newCDPSession(page);
  const send = (type: 'touchStart' | 'touchMove' | 'touchEnd', x = 0, y = 0) =>
    cdp.send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x, y }] });
  return {
    /** Finger down at (x0, y0), slides to (x1, y1) in `steps`, stays `holdMs` (runs `during`), lifts. */
    async drag(x0: number, y0: number, x1: number, y1: number, holdMs = 0, steps = 8, during?: () => Promise<void>) {
      await send('touchStart', x0, y0);
      for (let i = 1; i <= steps; i++) {
        await send('touchMove', x0 + ((x1 - x0) * i) / steps, y0 + ((y1 - y0) * i) / steps);
        await page.waitForTimeout(16);
      }
      if (holdMs) await page.waitForTimeout(holdMs);
      await during?.();
      await send('touchEnd');
    },
    async press(x: number, y: number, ms: number) {
      await send('touchStart', x, y);
      await page.waitForTimeout(ms);
      await send('touchEnd');
    },
  };
}

async function center(page: Page, selector: string): Promise<{ x: number; y: number }> {
  const box = (await page.locator(selector).boundingBox())!;
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

async function position(page: Page): Promise<{ x: number; y: number }> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

test('touch: joystick, skill buttons, auto-cast and the page swipe', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  // Skills are set up with the keyboard; the rest is touch only.
  await page.keyboard.press('KeyK');
  await learn(page, 'Fire Bolt', 1);
  await page.locator('.ge-tree header button', { hasText: 'Basic' }).click();
  await learn(page, 'Back Slide', 1);
  await slot(page, 'Back Slide', 0);
  await page.locator('.ge-tree header button', { hasText: 'Mage' }).click();
  await slot(page, 'Fire Bolt', 5);
  await page.keyboard.press('Escape');

  // The first touch turns the controls on (no coarse pointer on desktop).
  const t = await touch(page);
  await t.press(640, 120, 50);
  await expect(page.locator('.ge-touch')).toBeVisible();
  await expect(page.locator('.ge-hk-row').first()).toBeHidden();

  // Floating joystick: push right for a second; the character walks.
  const before = await position(page);
  await t.drag(300, 600, 380, 600, 1000);
  const after = await position(page);
  expect(Math.hypot(after.x - before.x, after.y - before.y)).toBeGreaterThan(1);

  await page.getByRole('button', { name: 'Start' }).tap();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);

  // Tap the first arc button (slot 1): Back Slide (needs no target, auto
  // aim) slides the character.
  const slot1 = '.ge-touch .tb[data-slot="0"]';
  await expect(page.locator(`${slot1} img`)).toHaveAttribute('src', /\.png$/);
  const s1 = await center(page, slot1);
  const from = await position(page);
  await expect
    .poll(
      async () => {
        await t.press(s1.x, s1.y, 60);
        const p = await position(page);
        return Math.hypot(p.x - from.x, p.y - from.y);
      },
      { timeout: 30_000, intervals: [500] },
    )
    .toBeGreaterThan(1);

  // Drag from it: the aim guide shows while the finger is down.
  await t.drag(s1.x, s1.y, s1.x - 90, s1.y - 40, 300, 1, async () => {
    await info.attach('aim', { body: await page.screenshot(), contentType: 'image/png' });
  });

  // Long-press: auto-cast on, marked on the touch button and the hotbar.
  // (A slow software-rendered frame can stretch one of the taps above
  // into a long press: start from off.)
  const auto = () => page.locator(slot1).evaluate((el) => el.classList.contains('auto'));
  if (await auto()) {
    await t.press(s1.x, s1.y, 900);
    await expect.poll(auto).toBe(false);
  }
  await t.press(s1.x, s1.y, 900);
  await expect.poll(auto).toBe(true);
  await expect(page.locator('.ge-hk.auto')).toHaveCount(1);
  const saved = await page.evaluate(() => JSON.parse(localStorage.getItem('game.settings') ?? '{}').autoCast);
  expect(saved).toBe(1);

  // Swipe along the arc ring: the second page (Fire Bolt in slot 6).
  const pivot = await center(page, '.ge-touch .tb.big');
  await t.drag(pivot.x - 150, pivot.y + 10, pivot.x - 110, pivot.y - 110, 0, 10);
  await expect(page.locator('.ge-touch .page b')).toHaveText('2');
  await expect(page.locator(slot1)).not.toHaveClass(/\bauto\b/);

  await info.attach('touch', { body: await page.screenshot(), contentType: 'image/png' });
  expect(errors).toEqual([]);
});
