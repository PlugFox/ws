import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, learn, signUp, slot, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

/** Standard-mapping buttons (Xbox names). */
const A = 0, B = 1, X = 2, LB = 4, RT = 7, VIEW = 8, MENU = 9, DOWN = 13;

/** A scripted standard gamepad behind `navigator.getGamepads()`. */
async function fakePad(page: Page) {
  await page.addInitScript(() => {
    const pad = {
      id: 'e2e pad',
      index: 0,
      connected: true,
      mapping: 'standard',
      timestamp: 0,
      buttons: Array.from({ length: 17 }, () => ({ pressed: false, touched: false, value: 0 })),
      axes: [0, 0, 0, 0],
    };
    (window as unknown as { __pad: typeof pad }).__pad = pad;
    Object.defineProperty(navigator, 'getGamepads', { value: () => [pad, null, null, null] });
  });
  const set = (buttons: number[], axes: number[] = [0, 0, 0, 0]) =>
    page.evaluate(
      ([b, a]) => {
        const pad = (window as unknown as { __pad: { buttons: { pressed: boolean; value: number }[]; axes: number[] } }).__pad;
        pad.buttons.forEach((x, i) => {
          x.pressed = b.includes(i);
          x.value = x.pressed ? 1 : 0;
        });
        pad.axes = a;
      },
      [buttons, axes] as const,
    );
  // The game polls the pad once per input tick from its frame loop: wait
  // for frames, not wall time (software rendering can be slow).
  const frames = (n: number) =>
    page.evaluate(
      (count) =>
        new Promise<void>((done) => {
          let k = 0;
          const step = () => (++k >= count ? done() : requestAnimationFrame(step));
          requestAnimationFrame(step);
        }),
      n,
    );
  return {
    /** Holds `buttons` (and sticks) for `ms` and at least a few frames, then lets go of everything. */
    async press(buttons: number[], ms = 100, axes?: number[]) {
      await set(buttons, axes);
      await Promise.all([page.waitForTimeout(ms), frames(4)]);
      await set([]);
      await frames(3);
    },
  };
}

async function position(page: Page): Promise<{ x: number; y: number }> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

test('gamepad: menus, the lobby panel, moving, auto-cast and casting', async ({ page }) => {
  const errors = watchErrors(page);
  const pad = await fakePad(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  // View opens the skill tree, B closes it.
  await pad.press([VIEW]);
  await expect(page.locator('.u-skills')).toBeVisible();
  await pad.press([DOWN]);
  await expect(page.locator('.u-skills .ge-nav')).toHaveCount(1);
  await pad.press([B]);
  await expect(page.locator('.u-skills')).toBeHidden();

  // Menu opens the game menu; the D-pad walks it; B closes.
  const menu = page.locator('[data-win="dialog"]');
  await pad.press([MENU]);
  await expect(menu).toContainText('Character select');
  await pad.press([DOWN]);
  await pad.press([DOWN]);
  await expect(menu.locator('.ge-nav')).toHaveCount(1);
  await pad.press([B]);
  await expect(menu).toHaveCount(0);

  // Left stick: the character walks.
  const before = await position(page);
  await pad.press([], 1000, [1, 0, 0, 0]);
  const after = await position(page);
  expect(Math.hypot(after.x - before.x, after.y - before.y)).toBeGreaterThan(1);

  // A skill for slot 1 (set up with the keyboard and mouse).
  await page.keyboard.press('KeyK');
  await learn(page, 'Napalm Beat', 1);
  await learn(page, 'Blink', 1);
  await slot(page, 'Blink', 0);
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-skills')).toBeHidden();

  // The D-pad takes the lobby panel; A presses Start.
  await pad.press([DOWN]);
  await expect(page.locator('.u-panel .ge-nav')).toHaveText('Start');
  await pad.press([A]);
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);

  // LB + RT + X: auto-cast for slot 1, shown on the hotbar.
  await pad.press([LB, RT]);
  await pad.press([LB, RT, X]);
  await expect(page.locator('.u-hk.auto')).toHaveCount(1);
  await pad.press([LB, RT, X]);
  await expect(page.locator('.u-hk.auto')).toHaveCount(0);

  // X casts slot 1: Blink moves the character.
  const from = await position(page);
  await expect
    .poll(
      async () => {
        await pad.press([X], 120);
        const p = await position(page);
        return Math.hypot(p.x - from.x, p.y - from.y);
      },
      { timeout: 30_000, intervals: [500] },
    )
    .toBeGreaterThan(1);
  expect(errors).toEqual([]);
});
