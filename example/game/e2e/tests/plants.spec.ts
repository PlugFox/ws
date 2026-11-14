import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, hold, signUp, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';
/** Field flora kinds (recipe.toml, archetype "plant"). */
const PLANT = /_(plant|mushroom)$/;

interface Mob {
  kind: string;
  x: number;
  y: number;
}
interface Probe {
  x: number;
  y: number;
  mobs: Mob[];
}

function probe(page: Page): Promise<Probe> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

/** The nearest plant in view, if any. */
async function nearestPlant(page: Page): Promise<{ me: Probe; plant?: Mob; d: number }> {
  const me = await probe(page);
  let plant: Mob | undefined;
  let d = Infinity;
  for (const m of me.mobs.filter((m) => PLANT.test(m.kind))) {
    const dm = Math.hypot(m.x - me.x, m.y - me.y);
    if (dm < d) [plant, d] = [m, dm];
  }
  return { me, plant, d };
}

/** Keys that step toward a world offset (the field camera starts north-up). */
function keysToward(dx: number, dy: number): string[] {
  const keys: string[] = [];
  if (Math.abs(dx) > 1) keys.push(dx > 0 ? 'KeyD' : 'KeyA');
  if (Math.abs(dy) > 1) keys.push(dy > 0 ? 'KeyW' : 'KeyS');
  return keys;
}

test('a mission field grows plants: rooted, in view, on the grass', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);

  // The cursor rests on open ground away from the hero: no plant is
  // pointed at, so standing next to one does not cut it (COMBAT.md §4.10).
  await page.mouse.move(1000, 560);

  // Plants are planted when the stage is made, away from the arrival: walk
  // the field until one is in the light.
  const sweep = ['KeyW', 'KeyD', 'KeyS', 'KeyS', 'KeyA', 'KeyA', 'KeyW', 'KeyW'];
  let found = await nearestPlant(page);
  for (let i = 0; !found.plant && i < sweep.length * 2; i++) {
    await hold(page, sweep[i % sweep.length], 2500);
    found = await nearestPlant(page);
  }
  expect(found.plant, 'a plant in view somewhere on the field').toBeTruthy();

  // Walk up to it (around trees: one axis at a time when stuck).
  for (let i = 0, last = found.me; i < 40 && found.plant && found.d > 4; i++) {
    let keys = keysToward(found.plant.x - found.me.x, found.plant.y - found.me.y);
    const stuck = i > 0 && Math.hypot(found.me.x - last.x, found.me.y - last.y) < 0.5;
    if (stuck && keys.length > 1) keys = [keys[i % 2]];
    last = found.me;
    for (const k of keys) await page.keyboard.down(k);
    await page.waitForTimeout(400);
    for (const k of keys) await page.keyboard.up(k);
    found = await nearestPlant(page);
  }
  const plant = found.plant!;
  info.annotations.push({ type: 'plant', description: `${plant.kind} at ${plant.x.toFixed(1)}, ${plant.y.toFixed(1)}, ${found.d.toFixed(1)} cells away` });

  // Rooted, and left alone: the same plant a second later.
  await page.waitForTimeout(1000);
  const again = (await probe(page)).mobs.find((m) => m.kind === plant.kind && Math.hypot(m.x - plant.x, m.y - plant.y) < 0.01);
  expect(again, `${plant.kind} stays where it grew`).toBeTruthy();

  await info.attach('plants', { body: await page.screenshot(), contentType: 'image/png' });
  await page.screenshot({ path: info.outputPath('plants.png') });
  expect(errors).toEqual([]);
});
