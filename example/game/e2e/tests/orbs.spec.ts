import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, learn, signUp, slot, watchErrors } from './helpers';

// The HP / SP orbs (engine kit/orb.ts): liquid glass that takes no clicks
// (potions are R / T and auto-use), a red pulse under 30 % HP, and on touch
// screens the bases of the sticks (touch.ts): dragging from the HP orb walks.
const MISSION = 'poring_island_survival';

/** Waits for the mission's map: loaded, its loading screen faded. */
async function inMission(page: Page): Promise<void> {
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  await expect
    .poll(() => page.evaluate(() => (JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()) as { loading: boolean }).loading))
    .toBe(false);
  await page.waitForTimeout(1500);
}

type Status = { hp: number; maxHp: number };
type View = { overlay: { status?: { last?: Status }; setStatus(s: Status): void }; setStatus(json: string): void };

/**
 * Before the page loads: keeps the view the client creates (the engine
 * sets `window.GameEngine = {createGameView, …}`) as `window.__view`, its
 * status feed wrapped so `window.__pin` (a fraction) overrides the shown HP.
 */
async function hookView(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as { __view?: View; __pin?: number };
    let engine: unknown;
    Object.defineProperty(window, 'GameEngine', {
      configurable: true,
      get: () => engine,
      set: (e: { createGameView?: () => View }) => {
        const make = e?.createGameView;
        if (typeof make !== 'function') {
          engine = e;
          return;
        }
        engine = {
          ...e,
          createGameView: () => {
            const view = make();
            const setStatus = view.setStatus.bind(view);
            view.setStatus = (json: string) => {
              const pin = w.__pin;
              if (json && pin !== undefined) {
                const s = JSON.parse(json) as Status;
                json = JSON.stringify({ ...s, hp: Math.round(s.maxHp * pin) });
              }
              setStatus(json);
            };
            return (w.__view = view);
          },
        };
      },
    });
  });
}

/**
 * Pins the local player's HP shown by the engine to `frac` of the max (the
 * view's status feed; the server's HP is untouched): a low-health look
 * without waiting for monsters to land hits.
 */
async function pinHp(page: Page, frac: number): Promise<void> {
  await page.evaluate((f) => {
    const w = window as unknown as { __view: View; __pin: number };
    w.__pin = f;
    const last = w.__view.overlay.status?.last;
    if (last) w.__view.overlay.setStatus({ ...last, hp: Math.round(last.maxHp * f) });
  }, frac);
}

test('orbs: liquid look at full and low HP, no click-to-drink', async ({ page }, info) => {
  const errors = watchErrors(page);
  await hookView(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await inMission(page);

  const hp = page.locator('.u-orb.hp');
  const sp = page.locator('.u-orb.sp');
  await expect(hp.locator('canvas')).toBeVisible();
  await expect(sp.locator('canvas')).toBeVisible();
  await expect(hp.locator('.u-orb-val')).toContainText(/HP\s*\d/);
  // Desktop: the orbs stay in the HUD's corners.
  const box = (await hp.boundingBox())!;
  expect(box.x).toBeLessThan(40);
  expect(box.y + box.height).toBeGreaterThan(700);
  await page.waitForTimeout(1200);
  await page.screenshot({ path: info.outputPath('orbs-full.png') });
  await hp.screenshot({ path: info.outputPath('orb-hp-full.png') });

  // The glass shows a tooltip, but there is nothing to click any more.
  await hp.locator('canvas').hover();
  const tip = page.locator('.u-tip');
  await expect(tip).toContainText(/HP \d+ \/ \d+/);
  await expect(tip).not.toContainText('Click');
  await expect(hp).not.toHaveCSS('cursor', 'pointer');
  await page.mouse.move(640, 300);

  // Low HP: the orb pulses red, the number turns red.
  await pinHp(page, 0.2);
  await expect(hp).toHaveClass(/\blow\b/);
  await page.waitForTimeout(1200);
  await page.screenshot({ path: info.outputPath('orbs-low.png') });
  await hp.screenshot({ path: info.outputPath('orb-hp-low.png') });
  await pinHp(page, 0.8);
  await expect(hp).not.toHaveClass(/\blow\b/);
  expect(errors).toEqual([]);
});

test.describe('on a phone', () => {
  test.use({ viewport: { width: 844, height: 390 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2 });

  test('orbs: the HP orb is the movement stick', async ({ page }, info) => {
    const errors = watchErrors(page);
    await signUp(page, `/?mission=${MISSION}`);
    await createCharacter(page, 'Mage');
    await enterWorld(page);
    // Fire Bolt on the big button (slot 5), set up with the keyboard.
    await page.keyboard.press('KeyK');
    await learn(page, 'Fire Bolt', 1);
    await page.locator('.u-skills .u-tabs button', { hasText: 'Mage' }).click();
    await slot(page, 'Fire Bolt', 4);
    await page.keyboard.press('Escape');

    const cdp = await page.context().newCDPSession(page);
    const touch = (type: 'touchStart' | 'touchMove' | 'touchEnd', x = 0, y = 0) =>
      cdp.send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x, y }] });
    // A touch turns the controls on (a coarse pointer already does).
    await touch('touchStart', 420, 80);
    await touch('touchEnd');
    await expect(page.locator('.ge-touch')).toBeVisible();
    await page.getByRole('button', { name: 'Start' }).tap();
    await inMission(page);

    // The orbs moved into the touch layer, in the bottom corners.
    const glass = page.locator('.ge-touch .u-orb.hp canvas');
    await expect(glass).toBeVisible();
    await expect(page.locator('.ge-touch .u-orb.sp canvas')).toBeVisible();
    const g = (await glass.boundingBox())!;
    const s = (await page.locator('.ge-touch .u-orb.sp canvas').boundingBox())!;
    expect(g.x).toBeLessThan(40);
    expect(g.y + g.height).toBeGreaterThan(370);
    expect(s.x + s.width).toBeGreaterThan(804);
    const cx = g.x + g.width / 2, cy = g.y + g.height / 2;
    // The big skill button sits on the SP orb's centre (the right stick).
    const big = page.locator('.ge-touch .tb.big');
    if (await big.isVisible()) {
      const b = (await big.boundingBox())!;
      expect(Math.abs(b.x + b.width / 2 - (s.x + s.width / 2))).toBeLessThan(3);
      expect(Math.abs(b.y + b.height / 2 - (s.y + s.height / 2))).toBeLessThan(3);
    }
    await page.waitForTimeout(800);
    await page.screenshot({ path: info.outputPath('phone.png') });

    // Dragging from the big button aims it, the SP orb's knob showing the pull.
    const sc = { x: s.x + s.width / 2, y: s.y + s.height / 2 };
    await touch('touchStart', sc.x, sc.y);
    for (let i = 1; i <= 6; i++) {
      await touch('touchMove', sc.x - i * 12, sc.y - i * 6);
      await page.waitForTimeout(16);
    }
    await page.waitForTimeout(300);
    await page.screenshot({ path: info.outputPath('phone-aim.png') });
    // Back onto the button and lift: cancelled, nothing cast.
    await touch('touchMove', sc.x, sc.y);
    await touch('touchEnd');

    const position = async () =>
      page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()) as { x: number; y: number });
    const before = await position();
    // Drag from the orb's centre to the right and hold: the hero walks.
    await touch('touchStart', cx, cy);
    for (let i = 1; i <= 6; i++) {
      await touch('touchMove', cx + i * 8, cy - i * 2);
      await page.waitForTimeout(16);
    }
    await page.waitForTimeout(400);
    await page.screenshot({ path: info.outputPath('phone-drag.png') });
    await glass.screenshot({ path: info.outputPath('phone-orb-drag.png') });
    await page.waitForTimeout(600);
    await touch('touchEnd');
    const after = await position();
    expect(after.x - before.x).toBeGreaterThan(1);
    // Not a tap on the world, and no tooltip from the long hold on the glass.
    await expect(page.locator('.u-tip')).toBeHidden();

    // Released: the hero stops.
    await page.waitForTimeout(400);
    const rest = await position();
    await page.waitForTimeout(500);
    const still = await position();
    expect(Math.hypot(still.x - rest.x, still.y - rest.y)).toBeLessThan(0.3);
    expect(errors).toEqual([]);
  });
});
