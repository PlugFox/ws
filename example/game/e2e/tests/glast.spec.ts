import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

/** Glast Heim Churchyard (docs/locations/glast_heim.md): the mission is
 * open, its stage is the churchyard of gl_chyard at night, only its own
 * dead come out, the Witching Hour (forced with the GAME_DEV `/event` hook
 * instead of waiting four minutes) is announced and curses the plots, a
 * Zombie Master (`/spawn`) raises fresh graves that open into Ghouls, and
 * the Dark Lord opens his fight. */
const MISSION = 'glast_heim';
/** Kinds the Glast Heim stage may show (recipe `director.missions`). */
const ROSTER = new Set([
  'ghoul', 'whisper', 'hunter_fly', 'skel_prisoner', 'raydric_archer', 'evil_druid', 'raydric', 'wraith', 'dark_priest',
  'khalitzburg', 'knight_of_abyss', 'zombie_master', 'dark_illusion', 'dark_illusion_image', 'bloody_knight', 'dark_lord',
  'grave', 'fresh_grave', 'holy_font', 'holy_font_dry',
]);
/** Interactive objects (not monsters that come out of the dark). */
const OBJECTS = new Set(['grave', 'fresh_grave', 'holy_font', 'holy_font_dry']);

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

test('Glast Heim: the dead of the churchyard, the Witching Hour and the Dark Lord', async ({ page }, info) => {
  test.setTimeout(150_000);
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Acolyte');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/, { timeout: 60_000 });
  // The timeline marks the Witching Hours and the eclipse.
  await expect(page.locator('.ge-tl')).toBeVisible();

  // A starting character does not last long here: everything is called up
  // at once. The bell tolls thirteen (the plots are cursed, the mist turns
  // violet), the Dark Lord rises, and a Zombie Master beside the party
  // raises fresh graves (they open into Ghouls).
  const kinds = new Set<string>();
  const look = async () => {
    for (const m of (await probe(page)).mobs) kinds.add(m.kind);
  };
  await page.screenshot({ path: info.outputPath('glast.png') });
  // The boss zones are meant to kill a level-30 priest who stands in
  // them: this test is about the place, so the hero is made sturdy.
  await say(page, '/sturdy');
  await say(page, '/event witching_hour');
  await say(page, '/spawn zombie_master');
  await say(page, '/spawn dark_lord');
  await expect(page.locator('body')).toContainText('Witching Hour', { timeout: 10_000 });
  // The boss: his encounter bar shows.
  await expect.poll(async () => (await look(), kinds.has('dark_lord')), { timeout: 10_000 }).toBeTruthy();
  await expect(page.locator('body')).toContainText('Dark Lord', { timeout: 10_000 });
  await page.screenshot({ path: info.outputPath('glast_dark_lord.png') });
  // The Zombie Master's fresh graves.
  await expect.poll(async () => (await look(), kinds.has('fresh_grave')), { timeout: 12_000 }).toBeTruthy();
  await page.screenshot({ path: info.outputPath('glast_raise.png') });

  // Only the churchyard's own dead and objects showed up.
  info.annotations.push({ type: 'kinds', description: [...kinds].join(', ') });
  expect([...kinds].some((k) => !OBJECTS.has(k)), 'the dead are out').toBeTruthy();
  for (const k of kinds) expect(ROSTER.has(k), `${k} belongs to Glast Heim`).toBeTruthy();
  expect(errors).toEqual([]);
});
