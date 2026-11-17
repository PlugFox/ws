import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

/** Toy Factory (docs/locations/toy_factory.md): the mission is open, its
 * stage is the frozen factory of xmas_dun02, only its own toys come out,
 * a player standing on a conveyor belt rides it without input, a shift
 * change (forced with the GAME_DEV `/event` hook instead of waiting two
 * minutes) is announced and turns the belt round, and the Stormy Knight
 * (`/spawn`) opens his fight. */
const MISSION = 'toy_factory';
/** Kinds the Toy Factory stage may show (recipe `director.missions`). */
const ROSTER = new Set([
  'cookie', 'mystcase', 'cookie_xmas', 'cruiser', 'teddy_bear', 'chepet', 'marionette', 'alarm', 'clock', 'garm_baby',
  'antonio', 'foreman', 'hatii', 'knight_of_windstorm', 'factory_switch', 'factory_switch_off',
]);
const OBJECTS = new Set(['factory_switch', 'factory_switch_off']);
/** The belt lane west of the arrival hall: x 53..55, running +y (north). */
const LANE_X = 54;

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

test('Toy Factory: toys, a belt that carries you and the shift change', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Thief');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/, { timeout: 60_000 });
  // The timeline marks the shift changes.
  await expect(page.locator('.ge-tl')).toBeVisible();

  // Walk west onto the belt lane (short steps until the centre is on it).
  for (let i = 0; i < 40; i++) {
    const { x } = await probe(page);
    if (Math.abs(x - LANE_X) < 0.6) break;
    const key = x > LANE_X ? 'KeyA' : 'KeyD';
    await page.keyboard.down(key);
    await page.waitForTimeout(Math.abs(x - LANE_X) > 3 ? 300 : 60);
    await page.keyboard.up(key);
    await page.waitForTimeout(150);
  }
  const on = await probe(page);
  expect(Math.abs(on.x - LANE_X), `standing on the belt (x ${on.x})`).toBeLessThan(0.8);
  await page.waitForTimeout(400);
  const before = await probe(page);
  await page.waitForTimeout(2000);
  const after = await probe(page);
  info.annotations.push({ type: 'ride', description: `(${before.x}, ${before.y}) -> (${after.x}, ${after.y})` });
  expect(after.y - before.y, 'the belt carries the player north without input').toBeGreaterThan(2.5);
  await page.screenshot({ path: info.outputPath('toy_belt.png') });

  // The whistle: the shift is announced and the belt turns round.
  await say(page, '/event shift_change');
  await expect(page.locator('body')).toContainText('Shift Change', { timeout: 15_000 });
  await page.waitForTimeout(500);
  const s0 = await probe(page);
  await page.waitForTimeout(1500);
  const s1 = await probe(page);
  if (Math.abs(s0.x - LANE_X) < 1) expect(s1.y - s0.y, 'reversed: it carries the player south').toBeLessThan(-2);
  await page.screenshot({ path: info.outputPath('toy_shift.png') });

  // Only the factory's own toys come out.
  const kinds = new Set<string>();
  for (let i = 0; i < 12 && ![...kinds].some((k) => !OBJECTS.has(k)); i++) {
    for (const m of (await probe(page)).mobs) kinds.add(m.kind);
    if (![...kinds].some((k) => !OBJECTS.has(k))) await page.waitForTimeout(1500);
  }
  info.annotations.push({ type: 'kinds', description: [...kinds].join(', ') });
  expect([...kinds].some((k) => !OBJECTS.has(k)), 'toys come out of the halls').toBeTruthy();
  for (const k of kinds) expect(ROSTER.has(k), `${k} belongs to the Toy Factory`).toBeTruthy();

  // The boss, called up at once: his encounter bar shows.
  await say(page, '/spawn knight_of_windstorm');
  await expect.poll(async () => (await probe(page)).mobs.some((m) => m.kind === 'knight_of_windstorm'), { timeout: 15_000 }).toBeTruthy();
  await expect(page.locator('body')).toContainText('Stormy Knight', { timeout: 15_000 });
  await page.waitForTimeout(2000);
  await page.screenshot({ path: info.outputPath('toy_knight.png') });
  expect(errors).toEqual([]);
});
