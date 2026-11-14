import { expect, test } from '@playwright/test';
import { cooldown, createCharacter, enterWorld, learn, signUp, slot, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

test('a thief dusts the wave with poison and cloaks out of it', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  // The Thief is playable from the creation screen (no padlock, Create on).
  await createCharacter(page, 'Thief');
  await enterWorld(page);

  // Level 30 (GAME_DEV_LEVEL): the Assassin's tier is open.
  await page.keyboard.press('KeyK');
  await expect(page.locator('.u-skills')).toBeVisible();
  await learn(page, 'Envenom', 3);
  await learn(page, 'Venom Dust', 1);
  await learn(page, 'Hiding', 3);
  await learn(page, 'Cloaking', 1);
  await slot(page, 'Venom Dust', 0);
  await slot(page, 'Cloaking', 1);
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-skills')).toBeHidden();

  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);

  // Venom Dust waits for a group to land on; Cloaking goes off at once.
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
  await cast('Digit1', 'Venom Dust');
  await cast('Digit2', 'Cloaking');
  await page.waitForTimeout(800);
  await page.screenshot({ path: info.outputPath('venom.png') });
  expect(errors).toEqual([]);
});
