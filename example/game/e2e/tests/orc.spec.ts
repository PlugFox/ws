import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, hold, signUp, watchErrors } from './helpers';

/** Orc Village (docs/locations/orc_village.md): the mission is open, its
 * stage is the war camp of gef_fild10, only the warband's own roster comes
 * at the party (no slime from Poring Island) and the timeline marks the
 * war horns. (The horn itself sounds at 2:00, longer than a level-30 test
 * character lives here: its column, side and arrows are covered by the
 * server's `hub/orc_tests.rs` and the engine demo `demo/orc.html`.) */
const MISSION = 'orc_village';
/** Field flora on the grass (recipe `[locations.gef_fild10.plants]`). */
const PLANTS = ['green_plant', 'red_plant', 'yellow_plant', 'white_plant', 'blue_plant', 'red_mushroom', 'black_mushroom', 'shining_plant'];
/** Interactive objects and plants (not monsters that come at the party). */
const OBJECTS = new Set(['orc_banner', 'orc_drum', ...PLANTS]);
/** Kinds the Orc Village stage may show (recipe `director.missions`). */
const ROSTER = new Set([
  'orc_baby', 'ork_warrior', 'goblin_1', 'savage', 'orc_lady', 'orc_archer', 'zenorc', 'high_orc',
  'steam_goblin', 'goblin_leader', 'ork_hero', 'orc_lord', 'orc_banner', 'orc_drum',
  ...PLANTS,
]);

interface Probe {
  x: number;
  y: number;
  mobs: { kind: string; x: number; y: number }[];
}

function probe(page: Page): Promise<Probe> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

test('Orc Village: a stage of the warband, under the war horns', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Mage');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  // The run's timeline marks the map events: four war horns before the fog.
  await expect(page.locator('.ge-tl')).toBeVisible();
  await expect.poll(() => page.locator('.ge-tl .ev').count()).toBeGreaterThanOrEqual(4);

  // Orc babies and warriors come in the first waves; walk a little to meet them.
  const kinds = new Set<string>();
  const moving = () => [...kinds].some((k) => !OBJECTS.has(k));
  const sweep = ['KeyW', 'KeyD', 'KeyS', 'KeyA'];
  for (let i = 0; i < 16 && !moving(); i++) {
    for (const m of (await probe(page)).mobs) kinds.add(m.kind);
    if (!moving()) await hold(page, sweep[i % sweep.length], 1500);
  }
  info.annotations.push({ type: 'kinds', description: [...kinds].join(', ') });
  expect(moving(), 'the warband comes at the party').toBeTruthy();
  for (const k of kinds) expect(ROSTER.has(k), `${k} belongs to Orc Village`).toBeTruthy();
  await page.screenshot({ path: info.outputPath('orc.png') });

  expect(errors).toEqual([]);
});
