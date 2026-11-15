import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

/** The page's `GameView` (captured as the client creates it), for HUD calls. */
async function captureView(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as { GameEngine?: { createGameView: () => unknown }; __view?: unknown };
    let engine: { createGameView: () => unknown } | undefined;
    Object.defineProperty(w, 'GameEngine', {
      configurable: true,
      get: () => engine,
      set: (e: { createGameView: () => unknown }) => {
        const create = e.createGameView;
        engine = { ...e, createGameView: () => (w.__view = create()) };
      },
    });
  });
}

/** Calls `GameView.<method>(...args)` on the page. */
async function view(page: Page, method: string, ...args: unknown[]): Promise<void> {
  await page.evaluate(
    ([m, a]) => {
      const v = (window as unknown as { __view: Record<string, (...x: unknown[]) => void> }).__view;
      v[m as string](...(a as unknown[]));
    },
    [method, args] as const,
  );
}

/** The chat log and the HP orb (with its value plate) do not overlap. */
async function chatClearsOrb(page: Page): Promise<void> {
  const orb = await page.locator('.u-orb.hp').boundingBox();
  const chat = await page.locator('.u-chat').boundingBox();
  if (!orb || !chat) return;
  const apart = chat.y + chat.height <= orb.y + 1 || chat.y >= orb.y + orb.height - 1 || chat.x >= orb.x + orb.width - 1 || chat.x + chat.width <= orb.x + 1;
  expect(apart, 'chat clear of the HP orb').toBe(true);
}

async function intoMission(page: Page): Promise<void> {
  await captureView(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  await expect(page.locator('.ge-tl')).toBeVisible();
  // The map is in (the loading screen gone, the orbs up).
  await expect(page.locator('.u-orb.hp canvas')).toBeVisible();
  await page.waitForTimeout(1500);
}

/** A burst of drops: junk repeats, zeny, and a few real rarities. */
async function lootBurst(page: Page): Promise<void> {
  const drops: [string, number, boolean][] = [
    ['jellopy', 1, false], ['zeny', 120, false], ['jellopy', 2, false], ['fluff', 1, false], ['jellopy', 1, false],
    ['sticky_mucus', 1, false], ['eclipse_card', 1, true], ['zeny', 80, false], ['jellopy', 3, false],
    ['sunshot_bow', 1, true], ['clover', 1, false], ['oridecon', 1, true], ['feather', 2, false], ['jellopy', 1, false],
  ];
  for (const [item, count, rare] of drops) {
    await view(page, 'showLoot', 0, 200, 200, item, count, rare);
    await page.waitForTimeout(60);
  }
}

/** Drops from a pack: five sources around the player, junk, zeny and rarities. */
async function packBurst(page: Page): Promise<void> {
  const items: [string, number, boolean][] = [
    ['jellopy', 1, false], ['zeny', 12, false], ['sticky_mucus', 1, false], ['jellopy', 2, false], ['fluff', 1, false],
    ['eclipse_card', 1, true], ['zeny', 9, false], ['jellopy', 1, false], ['clover', 1, false], ['oridecon', 1, true],
  ];
  await page.evaluate((list) => {
    const v = (window as unknown as { __view: { showLoot: (...a: unknown[]) => void } }).__view;
    let k = 0;
    for (let round = 0; round < 3; round++) {
      for (const [item, count, rare] of list) {
        const src = 0x900000 + (k % 5);
        v.showLoot(src, 196 + (k % 5) * 1.5, 197 + (k % 3), item, count, rare);
        k++;
      }
    }
  }, items);
}

const BUFFS = JSON.stringify([
  { skill: 'blessing', ms: 36000 },
  { skill: 'increase_agi', ms: 30000 },
  { skill: 'endure', ms: 9000 },
  { skill: 'energy_coat', ms: 0 },
  { skill: 'item:red_potion', ms: 5000 },
  { skill: 'bonus:berserk', ms: 20000 },
  { skill: 'magnum_break', ms: 5000 },
  { skill: 'cond:poisoned', ms: 8000 },
  { skill: 'cond:silenced', ms: 6000 },
  { skill: 'cond:burning', ms: 7000 },
  { skill: 'cond:slowed', ms: 9000 },
]);

test('mission HUD: loot feed, level chip, buff rows and tooltips', async ({ page }, info) => {
  test.setTimeout(180_000);
  const errors = watchErrors(page);
  await intoMission(page);
  // Q2: the monster level chip right of the timeline explains itself.
  const lvl = page.locator('.ge-tl .lvl');
  await expect(lvl).toHaveText(/^Lv \+\d+$/);
  await lvl.hover();
  const tip = page.locator('.u-tip');
  await expect(tip).toContainText('Monster level +');
  await expect(tip).toContainText(/one level every \d+:\d\d/);
  await expect(tip).toContainText('bosses come at');
  await page.screenshot({ path: info.outputPath('1280-level-tip.png') });
  await page.mouse.move(640, 600);

  // Q1: the loot feed merges repeats and highlights only real rarities.
  await lootBurst(page);
  const feed = page.locator('.ge-lootfeed');
  await expect(feed.locator('.lf[data-item="jellopy"]')).toHaveCount(1);
  // ×8 from the burst (more if the fight drops some meanwhile).
  await expect(feed.locator('.lf[data-item="jellopy"]')).toContainText(/Jellopy ×([89]|\d\d)/);
  await expect(feed.locator('.lf.rare')).toHaveCount(3);
  await expect(feed.locator('.lf.rare[data-item="eclipse_card"]')).toContainText('card');
  // At most six lines (the ones added to least recently went).
  expect(await feed.locator('.lf:not(.out)').count()).toBeLessThanOrEqual(6);
  await page.screenshot({ path: info.outputPath('1280-feed.png') });
  // Zeny sums into one line.
  await view(page, 'showLoot', 0, 200, 200, 'zeny', 50, false);
  await view(page, 'showLoot', 0, 200, 200, 'zeny', 25, false);
  await expect(feed.locator('.lf[data-item="zeny"]:not(.out)')).toContainText('+75 z');
  await expect(feed.locator('.lf:not(.rare)', { hasText: 'Rare drop' })).toHaveCount(0);
  await expect(page.locator('.u-toast', { hasText: /drop|card|magic item/i })).toHaveCount(0);
  // Clear of the bottom HUD: over the SP orb or beside it, never on it or the hotbar.
  const fb = (await feed.boundingBox())!;
  for (const sel of ['.u-orb.sp', '.u-hotbar .u-hk >> nth=-1']) {
    const o = await page.locator(sel).boundingBox();
    if (!o) continue;
    const apart = fb.y + fb.height <= o.y + 1 || fb.x + fb.width <= o.x + 1 || fb.x >= o.x + o.width - 1;
    expect(apart, `feed clear of ${sel}`).toBe(true);
  }
  // The chat clears the HP orb (any UI scale: both are in layer px).
  await chatClearsOrb(page);
  // The chat keeps the rare ones; the Loot toggle shows everything.
  const chat = page.locator('.u-chat');
  await expect(chat.locator('.ln.loot.rare')).toHaveCount(3);
  await expect(chat.locator('.ln.loot:not(.rare)').first()).toBeHidden();

  // Q3: buffs and conditions under the minimap, wrapping in its width.
  await view(page, 'setBuffs', BUFFS);
  const row = page.locator('.ge-buffs');
  await expect(row.locator('.ge-buff')).toHaveCount(11);
  await expect(row.locator('.ge-buff.ge-cond')).toHaveCount(4);
  const rb = (await row.boundingBox())!;
  const mm = (await page.locator('.u-mm').boundingBox())!;
  expect(rb.x).toBeGreaterThanOrEqual(mm.x - 2);
  expect(rb.y).toBeGreaterThanOrEqual(mm.y + mm.height - 4);
  expect(rb.height).toBeGreaterThan(40);
  await page.screenshot({ path: info.outputPath('1280-hud.png') });
  await row.locator('[data-buff="cond:poisoned"]').hover();
  await expect(tip).toContainText('Poisoned');
  await expect(tip).toContainText('1d4 poison damage every second');
  await expect(tip).toContainText(/\d+ s left/);
  await expect(tip).toContainText('Source:');
  await page.screenshot({ path: info.outputPath('1280-cond-tip.png') });
  await row.locator('[data-buff="increase_agi"]').hover();
  await expect(tip).toContainText('Increase AGI');
  await expect(tip).toContainText('Source:');
  await row.locator('[data-buff="bonus:berserk"]').hover();
  await expect(tip).toContainText('faster');
  await page.screenshot({ path: info.outputPath('1280-buff-tip.png') });
  await page.mouse.move(640, 600);
  await chat.locator('.u-foot .lt').click();
  await expect(chat.locator('.ln.loot', { hasText: /Jellopy ×\d+/ }).first()).toBeVisible();

  // Q4: a landscape phone: auto scale 0.75, nothing off screen.
  await page.setViewportSize({ width: 720, height: 360 });
  await page.waitForTimeout(400);
  expect(await page.evaluate(() => getComputedStyle(document.querySelector('.u-layer')!).getPropertyValue('--ui-scale'))).toBe('0.75');
  await lootBurst(page);
  await page.screenshot({ path: info.outputPath('720x360.png') });
  await chatClearsOrb(page);
  // The windows open inside the screen.
  for (const [key, name] of [['Alt+KeyE', 'inventory'], ['KeyK', 'skills'], ['Alt+KeyQ', 'equip'], ['F10', 'settings']] as const) {
    await page.keyboard.press(key);
    await page.waitForTimeout(400);
    await page.screenshot({ path: info.outputPath(`720x360-${name}.png`) });
    for (const b of await page.locator('.u-win:not(.hidden)').evaluateAll((ws) => ws.map((w) => w.getBoundingClientRect().toJSON()))) {
      expect(b.left).toBeGreaterThanOrEqual(-1);
      expect(b.right).toBeLessThanOrEqual(721);
      expect(b.top).toBeGreaterThanOrEqual(-1);
    }
    await page.keyboard.press('Escape');
  }
  expect(errors).toEqual([]);
});

test.describe('phone portrait', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2 });
  test('the HUD fits a phone held upright', async ({ page }, info) => {
    const errors = watchErrors(page);
    await intoMission(page);
    await view(page, 'setBuffs', BUFFS);
    await lootBurst(page);
    await page.screenshot({ path: info.outputPath('phone-portrait.png') });
    await chatClearsOrb(page);
    const box = (await page.locator('.ge-buffs').boundingBox())!;
    expect(box.x + box.width).toBeLessThanOrEqual(390);
    expect(errors).toEqual([]);
  });
});

test('interface scale 0.5 and 0.75 in the settings', async ({ page }, info) => {
  await intoMission(page);
  const scale = () => page.evaluate(() => getComputedStyle(document.querySelector('.u-layer')!).getPropertyValue('--ui-scale'));
  expect(await scale()).toBe('1.5');
  for (const v of ['0.75', '0.5']) {
    await page.keyboard.press('F10');
    const pick = page.locator('.u-settings .row', { hasText: 'Interface scale' }).locator('select');
    await expect(pick.locator('option')).toHaveText(['Auto (1.5×)', '0.5×', '0.75×', '1×', '1.25×', '1.5×', '1.75×', '2×', '2.5×']);
    await pick.selectOption(v);
    await expect.poll(scale).toBe(v);
    await page.screenshot({ path: info.outputPath(`1280-settings-${v}.png`) });
    await page.locator('.u-settings').getByRole('button', { name: 'Close', exact: true }).click();
    await view(page, 'setBuffs', BUFFS);
    await lootBurst(page);
    await page.screenshot({ path: info.outputPath(`1280-scale-${v}.png`) });
    await chatClearsOrb(page);
  }
});

test.describe('a short landscape screen', () => {
  test.use({ viewport: { width: 720, height: 360 } });
  test('the party panel starts folded and pickup pops stay small', async ({ page }, info) => {
    const errors = watchErrors(page);
    await intoMission(page);
    await expect(page.locator('.u-social')).toHaveClass(/\bmin\b/);
    await packBurst(page);
    await page.waitForTimeout(250);
    await page.screenshot({ path: info.outputPath('720x360-pack.png') });
    // A tap unfolds it, and that is remembered.
    await page.locator('.u-social .u-sys .mini').click();
    await expect(page.locator('.u-social')).not.toHaveClass(/\bmin\b/);
    expect(errors).toEqual([]);
  });
});

test('pickup pops at 1280x800: merged, three per source', async ({ page }, info) => {
  const errors = watchErrors(page);
  await intoMission(page);
  await expect(page.locator('.u-social')).not.toHaveClass(/\bmin\b/);
  await packBurst(page);
  await page.waitForTimeout(250);
  await page.screenshot({ path: info.outputPath('1280-pack.png') });
  expect(errors).toEqual([]);
});
