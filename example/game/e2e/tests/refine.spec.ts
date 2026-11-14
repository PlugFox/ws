import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

// Refining (server combat/refine.rs, engine refine_window.ts): `?shop=hollgrehenn`
// opens the smith's window from the spawn (GAME_DEV). The GAME_DEV chat
// hooks fill the bag (`/give phracon`, `/give sword+7`, `/give zeny 20000`)
// and fix the smith's next roll (`/rig <d100> <slip>`), so a failure and its
// slip die are deterministic.

/**
 * Screenshots of the outcome while it plays: the browser renders too slowly
 * here (software WebGL) to catch a moment on its own, so once the stage
 * takes class `cls` the page waits `ms`, then holds the window still (its
 * timers queued, its CSS animations paused and moved on to `seek` ms, as
 * starved frames leave them behind) until `shoot` lets go.
 */
async function holdAt(page: Page, cls: string, ms: number, seek = ms): Promise<void> {
  await page.evaluate(
    ([cls, ms, seek]) => {
      type W = Window & { __hold?: boolean; __held?: (() => void)[]; __frozen?: boolean; __timer?: typeof setTimeout };
      const w = window as W;
      if (!w.__timer) {
        const timer = (w.__timer = window.setTimeout.bind(window));
        w.__held = [];
        (window as unknown as { setTimeout: unknown }).setTimeout = (fn: () => void, t?: number) => timer(() => (w.__hold ? w.__held!.push(fn) : fn()), t);
      }
      w.__frozen = false;
      const stage = document.querySelector('[data-win="refine"] .stage')!;
      const seen = new MutationObserver(() => {
        if (!stage.classList.contains(cls)) return;
        seen.disconnect();
        w.__timer!(() => {
          w.__hold = true;
          for (const a of document.querySelector('[data-win="refine"]')!.getAnimations({ subtree: true })) {
            a.pause();
            if (Number(a.currentTime) < seek) a.currentTime = seek;
          }
          w.__frozen = true;
        }, ms);
      });
      seen.observe(stage, { attributes: true, attributeFilter: ['class'] });
    },
    [cls, ms, seek] as const,
  );
}

async function shoot(page: Page, path: string): Promise<void> {
  await page.waitForFunction(() => (window as Window & { __frozen?: boolean }).__frozen);
  await page.locator('[data-win="refine"]').screenshot({ path });
  await page.evaluate(() => {
    const w = window as Window & { __hold?: boolean; __held?: (() => void)[] };
    w.__hold = false;
    for (const a of document.querySelector('[data-win="refine"]')!.getAnimations({ subtree: true })) a.play();
    for (const f of w.__held!.splice(0)) f();
  });
}

async function say(page: Page, text: string): Promise<void> {
  await page.keyboard.press('Enter');
  const input = page.locator('.u-chat input');
  await expect(input).toBeVisible();
  await input.fill(text);
  await input.press('Enter');
}

test('the smith refines the worn sword with Phracon', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, '/?shop=hollgrehenn');
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);

  const win = page.locator('[data-win="refine"]');
  await expect(win).toContainText('Hollgrehenn the Smith');
  const sword = win.locator('.row', { hasText: /^Sword/ });
  await expect(sword).toContainText('Weapon · worn');
  await expect(sword.locator('.odds')).toHaveText('safe');
  await sword.click();
  const strike = win.getByRole('button', { name: 'Strike' });
  // No Phracon yet: the strike is off, the footer says why.
  await expect(strike).toBeDisabled();
  await expect(win.locator('.u-foot .u-note')).toHaveText('You need 1 Phracon.');
  await expect(win.locator('.facts')).toContainText('1 Phracon(have 0)50 Z');
  await expect(win.locator('.facts')).toContainText('Cannot fail: +0 → +1');
  await expect(win.locator('.lv')).toHaveText('+0→+1');

  // An ore in the bag redraws the window.
  await say(page, '/give phracon');
  await expect(strike).toBeEnabled();
  await expect(win.locator('.facts')).toContainText('(have 1)');
  await page.mouse.move(0, 0);
  await win.screenshot({ path: info.outputPath('refine-window.png') });

  // The hammer winds up, lands, and the sword glows at +1.
  await holdAt(page, 'ok', 260);
  await strike.click();
  const stage = win.locator('.stage');
  await expect(stage).toHaveClass(/\bok\b/);
  await expect(win.locator('.pop')).toHaveText('+1');
  await shoot(page, info.outputPath('refine-success.png'));
  await expect(win.locator('.u-foot .u-note')).toHaveText('Success! Sword is now +1.');
  await expect(win.locator('.row.on')).toContainText('+1 Sword');
  await expect(win.locator('.lv')).toHaveText('+1→+2');
  await expect(strike).toBeDisabled();

  // The equipment window names it +1 and the tooltip says what it adds.
  await page.keyboard.press('Escape');
  await expect(win).toHaveCount(0);
  await page.keyboard.press('Alt+KeyQ');
  const eq = page.locator('[data-win="equip"]');
  await expect(eq.getByText('+1 Sword').first()).toBeVisible();
  await eq.locator('img[data-icon="item:sword"]').first().hover();
  await expect(page.locator('.u-tip')).toContainText('Refined +1: +1 damage');
  expect(errors).toEqual([]);
});

test('a failed strike rolls the slip die and a big one shines', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, '/?shop=hollgrehenn');
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  const win = page.locator('[data-win="refine"]');
  await expect(win).toContainText('Hollgrehenn the Smith');

  for (const t of ['/give sword+7', '/give oridecon', '/give zeny 20000']) await say(page, t);
  const risky = win.locator('.row', { hasText: '+7 Sword' });
  await expect(risky).toContainText('Bag');
  await expect(risky.locator('.odds')).toHaveText('70%');
  await risky.click();
  const facts = win.locator('.facts');
  await expect(facts).toContainText('70%');
  await expect(facts.locator('.shape')).toHaveText('d4');
  await expect(facts.locator('.faces')).toHaveText('1→+62→+53→+44→+3');
  await expect(win.locator('.track .pip.next')).toHaveText('+8');
  await page.mouse.move(0, 0);
  await win.screenshot({ path: info.outputPath('refine-risky.png') });

  // The smith's next roll: 100 fails the 70%, slip 3 shows a 3 on the d4.
  await say(page, '/rig 100 3');
  // Held just after the die lands (600 ms in, a 900 ms tumble).
  await holdAt(page, 'fail', 1750, 500);
  await win.getByRole('button', { name: 'Strike' }).click();
  const stage = win.locator('.stage');
  await expect(stage).toHaveClass(/\bfail\b/);
  const die = win.locator('.die');
  await expect(die).toHaveClass(/landed/);
  await expect(die).toHaveText('3');
  await expect(win.locator('.pop')).toHaveText('−3');
  await shoot(page, info.outputPath('refine-failure.png'));
  await expect(win.locator('.u-foot .u-note')).toHaveText('Failed: the die showed 3, Sword fell to +4.');
  await expect(win.locator('.row.on')).toContainText('+4 Sword');
  await expect(stage).toHaveClass(/\blost\b/);

  // +6 → +7 is safe for a level-1 sword and staged as a big win; Space strikes.
  for (const t of ['/give sword+6', '/give emveretarcon']) await say(page, t);
  await win.locator('.row', { hasText: '+6 Sword' }).click();
  await page.mouse.move(0, 0);
  await holdAt(page, 'great', 420);
  await page.keyboard.press('Space');
  await expect(stage).toHaveClass(/\bgreat\b/);
  await shoot(page, info.outputPath('refine-great.png'));
  // Space again skips to the outcome.
  await page.keyboard.press('Space');
  await expect(win.locator('.u-foot .u-note')).toHaveText('Success! Sword is now +7.');
  await expect(win.locator('.row.on')).toContainText('+7 Sword');
  expect(errors).toEqual([]);
});
