import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, hold, signUp, watchErrors } from './helpers';

/** Prontera Culvert (docs/locations/culvert.md): the mission is open, its
 * stage is the sewers of prt_sewb1, its own roster crawls in (no slime from
 * Poring Island), the timeline shows the flushes, and the Golden Thief Bug
 * (spawned with the GAME_DEV `/spawn` hook) opens its fight. */
const MISSION = 'prontera_culvert';
/** Kinds the culvert stage may show (recipe `director.missions`). */
const ROSTER = new Set([
  'thief_bug_larva', 'thief_bug', 'tarou', 'plankton', 'thief_bug_female', 'thief_bug_male', 'drainliar',
  'thief_bug_egg', 'thief_bug_queen', 'rat_king', 'golden_bug',
]);

interface Probe {
  x: number;
  y: number;
  mobs: { kind: string; x: number; y: number }[];
}

function probe(page: Page): Promise<Probe> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

async function say(page: Page, text: string): Promise<void> {
  await page.keyboard.press('Enter');
  const input = page.locator('.u-chat input');
  await expect(input).toBeVisible();
  await input.fill(text);
  await input.press('Enter');
}

test('Prontera Culvert: bugs and rats in the sewers, and the Golden Thief Bug', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/, { timeout: 60_000 });
  // The timeline marks the map events (the flushes, west then east).
  await expect(page.locator('.ge-tl')).toBeVisible();

  // Larvae come first, Thief Bugs at 0:20; walk a little to meet them.
  const kinds = new Set<string>();
  const moving = () => [...kinds].some((k) => k !== 'thief_bug_egg');
  const sweep = ['KeyW', 'KeyD', 'KeyS', 'KeyA'];
  for (let i = 0; i < 16 && !moving(); i++) {
    for (const m of (await probe(page)).mobs) kinds.add(m.kind);
    if (!moving()) await hold(page, sweep[i % sweep.length], 1500);
  }
  info.annotations.push({ type: 'kinds', description: [...kinds].join(', ') });
  expect(moving(), 'monsters crawl out of the sewers').toBeTruthy();
  for (const k of kinds) expect(ROSTER.has(k), `${k} belongs to the culvert`).toBeTruthy();

  // The boss, called up at once: it yells and its encounter bar shows.
  await say(page, '/spawn golden_bug');
  await expect.poll(async () => (await probe(page)).mobs.some((m) => m.kind === 'golden_bug'), { timeout: 15_000 }).toBeTruthy();
  await expect(page.locator('body')).toContainText('Golden Thief Bug', { timeout: 15_000 });
  await page.screenshot({ path: info.outputPath('culvert.png') });
  expect(errors).toEqual([]);
});
