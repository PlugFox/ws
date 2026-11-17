import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, hold, signUp, watchErrors } from './helpers';

/** Payon Caves (docs/locations/payon.md): the mission is open, its stage
 * is the dark cave of pay_dun04 and only its own roster comes out of the
 * dark (no slime from Poring Island). */
const MISSION = 'payon_caves';
/** Kinds the Payon stage may show (recipe `director.missions`). */
const ROSTER = new Set([
  'zombie', 'skeleton', 'archer_skeleton', 'familiar', 'horong', 'munak', 'bongun', 'sohee', 'nine_tail',
  'weeping_bride', 'jiangshi_general', 'moonlight_flower', 'moonlight_mirror', 'brazier', 'shrine_bell', 'shrine_bell_quiet',
]);

/** Interactive objects (not monsters that come out of the dark). */
const OBJECTS = new Set(['brazier', 'shrine_bell', 'shrine_bell_quiet']);

interface Probe {
  x: number;
  y: number;
  mobs: { kind: string; x: number; y: number }[];
}

function probe(page: Page): Promise<Probe> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

test('Payon Caves: a dark stage of its own dead', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Acolyte');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);

  // The first undead shamble in within the first waves; walk a little to
  // meet them in the dark.
  const kinds = new Set<string>();
  const sweep = ['KeyW', 'KeyD', 'KeyS', 'KeyA'];
  for (let i = 0; i < 16 && ![...kinds].some((k) => !OBJECTS.has(k)); i++) {
    for (const m of (await probe(page)).mobs) kinds.add(m.kind);
    if (![...kinds].some((k) => !OBJECTS.has(k))) await hold(page, sweep[i % sweep.length], 1500);
  }
  info.annotations.push({ type: 'kinds', description: [...kinds].join(', ') });
  expect([...kinds].some((k) => !OBJECTS.has(k)), 'monsters come out of the dark').toBeTruthy();
  for (const k of kinds) expect(ROSTER.has(k), `${k} belongs to Payon Caves`).toBeTruthy();

  await info.attach('payon', { body: await page.screenshot(), contentType: 'image/png' });
  expect(errors).toEqual([]);
});
