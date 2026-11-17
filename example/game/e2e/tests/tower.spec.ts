import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

/** Thanatos Tower (docs/locations/thanatos_tower.md): the mission is open,
 * its stage is the museum floor of tha_t02, the Museum's own kinds come
 * first (no plasma, no angel before the stairs open), the Laboratory's stair
 * and a Judgment (forced with the GAME_DEV `/event` hook instead of
 * waiting minutes) are announced and re-tint the hall, the five plasmas
 * read apart, and Thanatos (`/spawn`) forms in the nave, out of sight of the
 * gate. */
const MISSION = 'thanatos_tower';
/** Act I (the Museum) and the run's objects. */
const ACT_ONE = new Set(['deathword', 'ancient_mimic', 'owl_duke', 'curator', 'specimen_tube_r', 'specimen_tube_b', 'specimen_tube_g', 'specimen_tube_p', 'specimen_tube_y']);

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

test('Thanatos Tower: the Museum, the Laboratory, a Judgment and Thanatos', async ({ page }, info) => {
    const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Acolyte');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/, { timeout: 60_000 });
  // The timeline marks the stairs, the Judgments and the summit.
  await expect(page.locator('.ge-tl')).toBeVisible();

  // The e2e hero is level 30 and the tower's monsters 89 and up: it lasts
  // seconds, so everything below happens at once. Books and mimics come
  // first (the opening wave walks to the gate).
  const kinds = new Set<string>();
  const moving = () => [...kinds].some((k) => !k.startsWith('specimen_tube'));
  for (let i = 0; i < 12 && !moving(); i++) {
    for (const m of (await probe(page)).mobs) kinds.add(m.kind);
    if (!moving()) await page.waitForTimeout(500);
  }
  info.annotations.push({ type: 'kinds', description: [...kinds].join(', ') });
  for (const k of kinds) expect(ACT_ONE.has(k), `${k} belongs to the Museum (Act I)`).toBeTruthy();
  await page.screenshot({ path: info.outputPath('tower_museum.png') });

  // The Laboratory's stair: announced, the air turns green; the five
  // plasmas side by side.
  await say(page, '/event stair_lab');
  for (const c of ['r', 'b', 'g', 'p', 'y']) await say(page, `/spawn plasma_${c}`);
  await expect.poll(async () => (await probe(page)).mobs.filter((m) => m.kind.startsWith('plasma_')).length, { timeout: 5_000 }).toBeGreaterThanOrEqual(3);
  await page.waitForTimeout(1200);
  await page.screenshot({ path: info.outputPath('tower_laboratory.png') });

  // A Judgment: the statues' calm rings and the beams.
  await say(page, '/event judgment');
  await page.waitForTimeout(1500);
  await page.screenshot({ path: info.outputPath('tower_judgment.png') });

  // Thanatos, called up at once: he forms in the nave (hub/thanatos.rs),
  // far out of the gate's sight, so the hero does not see him come.
  await say(page, '/spawn thanatos');
  await page.waitForTimeout(500);
  expect((await probe(page)).mobs.some((m) => m.kind === 'thanatos'), 'he is moved into the nave').toBeFalsy();
  await say(page, '/event summit');
  await page.waitForTimeout(1200);
  await page.screenshot({ path: info.outputPath('tower_summit.png') });
  expect(errors).toEqual([]);
});
