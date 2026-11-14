import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, hold, signUp, watchErrors } from './helpers';

/** Sograt Desert (docs/locations/sograt.md): the mission is open, its stage
 * is the dunes of moc_fild17, the desert's own roster comes over the sand
 * (no slime from Poring Island) and the timeline shows the sandstorms. */
const MISSION = 'sograt_desert';
/** Kinds the Sograt stage may show (recipe `director.missions`). */
const ROSTER = new Set([
  'picky', 'condor', 'desert_wolf_b', 'scorpion', 'pecopeco', 'hode', 'frilldora', 'desert_wolf', 'sand_man',
  'sirocco', 'sand_colossus', 'phreeoni', 'muka', 'picky_egg',
]);

interface Probe {
  x: number;
  y: number;
  mobs: { kind: string; x: number; y: number }[];
}

function probe(page: Page): Promise<Probe> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

test('Sograt Desert: a stage of its own beasts, under the storms', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Archer');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  // The run's timeline marks the map events (the sandstorm every 2½ min).
  await expect(page.locator('.ge-tl')).toBeVisible();

  // Picky chicks and condors come in the first waves (Muka cacti stand
  // around from the start); walk a little to meet them.
  const kinds = new Set<string>();
  const moving = () => [...kinds].some((k) => k !== 'muka');
  const sweep = ['KeyW', 'KeyD', 'KeyS', 'KeyA'];
  for (let i = 0; i < 16 && !moving(); i++) {
    for (const m of (await probe(page)).mobs) kinds.add(m.kind);
    if (!moving()) await hold(page, sweep[i % sweep.length], 1500);
  }
  info.annotations.push({ type: 'kinds', description: [...kinds].join(', ') });
  expect(moving(), 'monsters come over the dunes').toBeTruthy();
  for (const k of kinds) expect(ROSTER.has(k), `${k} belongs to Sograt Desert`).toBeTruthy();

  await page.screenshot({ path: info.outputPath('sograt.png') });
  expect(errors).toEqual([]);
});
