import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

/** The mission window's own Start (the party panel has one too). */
const startIn = (page: Page) => page.locator('.u-missions .u-foot').getByRole('button', { name: 'Start', exact: true });

// Quick start (lobby.rs `quick_start`): the mission window's Start goes
// alone at once, with the window's modifiers; a party's leader starts the
// party's run at once (there is no ready check). `?mission=` (GAME_DEV)
// opens a party of one, whose Modifiers button opens the window without a
// walk to a Kafra. (A member's refused start, who stays behind:
// lobby.rs tests; the party outliving the run: party.spec.ts.)
test('Start in the mission window launches at once, alone or as a party leader', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  // The leader of a party of one may start it from the window.
  await page.getByRole('button', { name: 'Modifiers…' }).click();
  const win = page.locator('.u-missions');
  await expect(startIn(page)).toBeEnabled();
  await expect(win.locator('.mi-note')).toHaveCount(0);

  // Leaving the party (the window stays open): Start now goes alone, with
  // the window's own modifiers.
  await page.getByRole('button', { name: 'Leave', exact: true }).dispatchEvent('click');
  await expect(page.locator('body')).toContainText(/Find a party/);
  await win.locator('.mo-c', { hasText: 'Darkness' }).click();
  await win.locator('.mo-tier .u-range').fill('2');
  await expect(startIn(page)).toBeEnabled();
  await page.screenshot({ path: info.outputPath('solo.png') });
  await startIn(page).click();

  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  await expect(page.locator('.ge-tl .mods i.tier')).toHaveText('T2');
  await expect(page.locator('.ge-tl .mods i')).toHaveCount(2);
  // Toasts stack under the run's timeline, never over it.
  await expect(page.locator('.u-toast').first()).toBeVisible();
  const tl = await page.locator('.ge-tl').boundingBox();
  const toasts = await page.locator('.u-toasts').boundingBox();
  expect(toasts!.y).toBeGreaterThanOrEqual(tl!.y + tl!.height);
  await page.screenshot({ path: info.outputPath('stage.png') });
  expect(errors).toEqual([]);
});
