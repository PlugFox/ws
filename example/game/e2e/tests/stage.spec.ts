import { expect, test } from '@playwright/test';
import { cooldown, createCharacter, enterWorld, learn, signUp, slot, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

test('a mage lays a Fire Wall and a Quagmire on a mission', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  // Level 30 (GAME_DEV_LEVEL): learn the path to both ground skills in town.
  await page.keyboard.press('KeyK');
  await expect(page.locator('.ge-tree')).toBeVisible();
  await learn(page, 'Fire Bolt', 4);
  await learn(page, 'Sight', 1);
  await learn(page, 'Fire Wall', 1);
  await learn(page, 'Stone Curse', 1);
  await learn(page, "Heaven's Drive", 1);
  await learn(page, 'Quagmire', 1);
  await slot(page, 'Fire Wall', 0);
  await slot(page, 'Quagmire', 1);
  await page.keyboard.press('Escape');
  await expect(page.locator('.ge-tree')).toBeHidden();

  // `?mission=` picked the mission on the town join: a lobby of one.
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);

  // The wave comes to the mage; each skill fires once something is in reach
  // (no target = no cast), which starts its cooldown on the hotbar.
  const cast = async (key: string, name: string) => {
    await expect
      .poll(
        async () => {
          await page.keyboard.press(key);
          return cooldown(page, name);
        },
        { timeout: 60_000, intervals: [500] },
      )
      .toBeGreaterThan(0);
  };
  await cast('KeyE', 'Quagmire');
  await cast('KeyQ', 'Fire Wall');
  await page.waitForTimeout(1200);
  await info.attach('ground', { body: await page.screenshot(), contentType: 'image/png' });
  await page.screenshot({ path: info.outputPath('ground.png') });
  expect(errors).toEqual([]);
});
