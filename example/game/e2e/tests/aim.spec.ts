import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, learn, signUp, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

async function position(page: Page): Promise<{ x: number; y: number; loading: boolean; hp: number }> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()));
}

/** Settings → Manual aiming is kept with the other settings; travel skills
 * (Space) go where the hero walks, and standing still toward the pointer. */
test('manual aiming persists; Back Slide follows the walk, else the pointer', async ({ page }) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Archer');
  await enterWorld(page);

  // The setting: a toggle in the Settings window, saved in the browser.
  await page.keyboard.press('F10');
  const toggle = page.locator('.u-settings').getByRole('checkbox', { name: /^Manual aiming/ });
  await expect(toggle).toBeVisible();
  // The box itself is a styled stand-in; the input sits off-screen.
  await toggle.dispatchEvent('click');
  await expect
    .poll(() => page.evaluate(() => JSON.parse(localStorage.getItem('game.settings') ?? '{}').manualAim))
    .toBe(true);
  await page.keyboard.press('Escape');

  await page.keyboard.press('KeyK');
  await learn(page, 'Back Slide', 1);
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-skills')).toBeHidden();
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  await page.waitForTimeout(1500);

  // Arrival: the loading screen, then the server places the hero (party
  // ring): wait until the stage is up and the hero stands.
  await expect(async () => {
    const a = await position(page);
    await page.waitForTimeout(400);
    const b = await position(page);
    expect(b.loading).toBe(false);
    expect(Math.hypot(b.x - a.x, b.y - a.y)).toBeLessThan(0.01);
  }).toPass({ timeout: 15_000 });

  const vp = page.viewportSize()!;
  const walk = async (ms: number, during?: () => Promise<void>) => {
    const a = await position(page);
    await page.keyboard.down('KeyD');
    await page.waitForTimeout(ms / 2);
    await during?.();
    await page.waitForTimeout(ms / 2);
    await page.keyboard.up('KeyD');
    await page.waitForTimeout(400);
    const b = await position(page);
    return { x: b.x - a.x, y: b.y - a.y };
  };
  // Walking alone: the reference direction and distance (a poring's
  // freeze or stun can hold the hero for a moment: walk again then).
  let plain = { x: 0, y: 0 };
  await expect(async () => {
    plain = await walk(600);
    expect(Math.hypot(plain.x, plain.y)).toBeGreaterThan(0.5);
  }).toPass({ timeout: 15_000 });
  const len = Math.hypot(plain.x, plain.y);
  const along = (d: { x: number; y: number }) => (d.x * plain.x + d.y * plain.y) / len;

  // Walking right with the pointer on the left: the slide goes right.
  await page.mouse.move(vp.width * 0.1, vp.height / 2);
  const slid = await walk(600, () => page.keyboard.press('Space'));
  expect(along(slid)).toBeGreaterThan(len + 2);

  // Standing still, facing the pointer on the right: toward the pointer
  // (the old Back Slide went away from the facing). Walk left first and
  // turn right, so the slide runs back over ground just crossed (a stage
  // spawn may stand right next to a tree).
  await page.waitForTimeout(6000); // the cooldown
  await page.keyboard.down('KeyA');
  await page.waitForTimeout(1500);
  await page.keyboard.up('KeyA');
  await page.keyboard.down('KeyD');
  await page.waitForTimeout(120);
  await page.keyboard.up('KeyD');
  await page.waitForTimeout(500);
  await page.mouse.move(vp.width * 0.9, vp.height / 2);
  const a = await position(page);
  await page.keyboard.press('Space');
  await page.waitForTimeout(500);
  const b = await position(page);
  expect(along({ x: b.x - a.x, y: b.y - a.y })).toBeGreaterThan(2);
  expect(errors).toEqual([]);
});
