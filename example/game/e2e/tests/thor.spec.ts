import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, hold, signUp, watchErrors } from './helpers';

/** Thor Volcano (docs/locations/thor_volcano.md): the mission is open, its
 * stage is the magma chambers of thor_v03, only its own fire spirits and
 * beasts come out, a Lava Surge (forced with the GAME_DEV `/event` hook
 * instead of waiting 2½ minutes) is announced and floods the east
 * chambers, and Salamander (`/spawn`) opens his fight. */
const MISSION = 'thor_volcano';
/** Kinds the Thor stage may show (recipe `director.missions`). */
const ROSTER = new Set([
  'magmaring', 'explosion', 'gig', 'kaho', 'deleter_', 'imp', 'blazzer', 'lava_golem', 'nightmare_terror', 'byorgue',
  'sky_deleter', 'salamander', 'kasa', 'kasa_ember', 'ifrit',
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

test('Thor Volcano: fire spirits, Salamander and a lava surge', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Mage');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/, { timeout: 60_000 });
  // The timeline marks the surges and the tremor.
  await expect(page.locator('.ge-tl')).toBeVisible();

  // Magmarings come first; walk a little to meet them.
  const kinds = new Set<string>();
  const sweep = ['KeyW', 'KeyD', 'KeyS', 'KeyA'];
  for (let i = 0; i < 16 && kinds.size === 0; i++) {
    for (const m of (await probe(page)).mobs) kinds.add(m.kind);
    if (kinds.size === 0) await hold(page, sweep[i % sweep.length], 1500);
  }
  info.annotations.push({ type: 'kinds', description: [...kinds].join(', ') });
  expect(kinds.size > 0, 'monsters come out of the cracks').toBeTruthy();
  for (const k of kinds) expect(ROSTER.has(k), `${k} belongs to Thor Volcano`).toBeTruthy();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: info.outputPath('thor.png') });

  // The east chambers flood (the arrival chamber is west: a test character
  // far under the band would not live through its own half's surge).
  await say(page, '/event surge_east');
  await expect(page.locator('body')).toContainText('Lava Surge', { timeout: 15_000 });
  await page.waitForTimeout(2000);
  await page.screenshot({ path: info.outputPath('thor_surge.png') });

  // Salamander, called up at once (a scripted elite): he hisses his intro
  // and his encounter bar shows.
  await say(page, '/spawn salamander');
  await expect.poll(async () => (await probe(page)).mobs.some((m) => m.kind === 'salamander'), { timeout: 15_000 }).toBeTruthy();
  await expect(page.locator('body')).toContainText('Salamander', { timeout: 15_000 });
  await page.waitForTimeout(1200);
  await page.screenshot({ path: info.outputPath('thor_salamander.png') });

  expect(errors).toEqual([]);
});
