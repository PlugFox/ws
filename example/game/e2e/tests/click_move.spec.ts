import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

interface Debug {
  map: string;
  loading: boolean;
  x: number;
  y: number;
  walking: string | null;
  walkEnd?: [number, number];
  npcs: { id: string; name: string; x: number; y: number }[];
}

async function debug(page: Page): Promise<Debug> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

/** The client point whose ground is nearest to cell (x, y): a coarse scan
 * of the canvas, then a fine one around the best hit (`gameGroundAt`). */
async function screenOf(page: Page, x: number, y: number): Promise<{ x: number; y: number; d: number }> {
  return page.evaluate(
    ([tx, ty]) => {
      const at = (window as unknown as { gameGroundAt: (x: number, y: number) => [number, number] | null }).gameGroundAt;
      let best = { x: 0, y: 0, d: Infinity };
      const scan = (x0: number, y0: number, x1: number, y1: number, step: number) => {
        for (let sy = y0; sy <= y1; sy += step) {
          for (let sx = x0; sx <= x1; sx += step) {
            const g = at(sx, sy);
            if (!g) continue;
            const d = Math.hypot(g[0] - tx, g[1] - ty);
            if (d < best.d) best = { x: sx, y: sy, d };
          }
        }
      };
      // Clear of the HUD at the edges.
      scan(160, 120, innerWidth - 160, innerHeight - 160, 24);
      const b = best;
      scan(b.x - 24, b.y - 24, b.x + 24, b.y + 24, 3);
      return best;
    },
    [x, y],
  );
}

const dist = (a: { x: number; y: number }, x: number, y: number) => Math.hypot(a.x - x, a.y - y);

/** Click to move (COMBAT.md §4.2): a left click on the ground walks there
 * around obstacles, a click on an NPC walks to it and talks, and the
 * setting turns it off. */
test('click to move: walk to a cell, walk to an NPC and talk, off in the settings', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  await expect.poll(async () => (await debug(page)).loading).toBe(false);
  await page.waitForTimeout(500);
  const shots = process.env.CLICK_MOVE_SHOTS;

  // The ground a few cells south-west of the spawn.
  const start = await debug(page);
  const goal = { x: start.x - 5, y: start.y - 4 };
  const spot = await screenOf(page, goal.x, goal.y);
  expect(spot.d, 'the cell is on screen').toBeLessThan(1);
  await page.mouse.move(spot.x, spot.y);
  await page.mouse.down();
  await page.mouse.up();
  const planned = await debug(page);
  expect(planned.walking).toBe('ground');
  const [ex, ey] = planned.walkEnd!;
  // Open ground: the end is the clicked cell (or the walkable one next to it).
  expect(Math.hypot(ex - goal.x, ey - goal.y)).toBeLessThan(1.5);
  if (shots) await page.screenshot({ path: `${shots}/marker.png` });
  // Done, at the end. Within a cell: the server keeps one move per tick,
  // so in a burst of inputs (a busy headless frame) a step or two is lost.
  await expect.poll(async () => (await debug(page)).walking).toBeNull();
  const done = await debug(page);
  expect(dist(done, ex, ey), `at ${done.x}, ${done.y}`).toBeLessThan(1);
  await info.attach('walked', { body: await page.screenshot(), contentType: 'image/png' });

  // An NPC out of reach: the hero walks over and the talk opens.
  const here = await debug(page);
  const npc = here.npcs
    .filter((n) => n.id === 'cuskoal' || n.id === 'guard' || n.id === 'charfri')
    .sort((a, b) => dist(here, a.x, a.y) - dist(here, b.x, b.y))[0];
  expect(npc, 'a townsperson to talk to').toBeTruthy();
  expect(dist(here, npc.x, npc.y), 'out of reach first').toBeGreaterThan(3);
  // Its feet: the sprite stands on them.
  const feet = await screenOf(page, npc.x, npc.y);
  expect(feet.d).toBeLessThan(1);
  await page.mouse.move(feet.x, feet.y - 20);
  if (shots) await page.screenshot({ path: `${shots}/talk-cursor.png` });
  await page.mouse.click(feet.x, feet.y - 20);
  await expect(page.locator('[data-win="dialog"]')).toContainText(npc.name, { timeout: 15_000 });
  expect(dist(await debug(page), npc.x, npc.y)).toBeLessThanOrEqual(3.05);
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-win="dialog"]')).toHaveCount(0);

  // Keys take over at once.
  const back = await debug(page);
  const away = await screenOf(page, back.x + 6, back.y);
  await page.mouse.click(away.x, away.y);
  expect((await debug(page)).walking).toBe('ground');
  await page.keyboard.down('KeyW');
  await page.waitForTimeout(150);
  expect((await debug(page)).walking).toBeNull();
  await page.keyboard.up('KeyW');

  // Settings → Click to move off: a click on the ground does nothing.
  await page.keyboard.press('F10');
  const toggle = page.locator('.u-settings').getByRole('checkbox', { name: /^Click to move/ });
  await expect(toggle).toBeVisible();
  await toggle.dispatchEvent('click');
  await expect
    .poll(() => page.evaluate(() => JSON.parse(localStorage.getItem('game.settings') ?? '{}').clickMove))
    .toBe(false);
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-settings')).toBeHidden();
  await page.waitForTimeout(300);
  const still = await debug(page);
  const there = await screenOf(page, still.x + 5, still.y + 3);
  await page.mouse.click(there.x, there.y);
  await page.waitForTimeout(1200);
  const after = await debug(page);
  expect(after.walking).toBeNull();
  expect(dist(after, still.x, still.y)).toBeLessThan(0.05);

  expect(errors).toEqual([]);
});
