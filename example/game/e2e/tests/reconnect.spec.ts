import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, hold, signUp, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

/**
 * Every WebSocket the page opens is kept in `__sockets`, so the test can
 * drop the connection; while `__wsBlock` is set, new sockets go to a dead
 * port (the server is unreachable) and the client keeps retrying.
 */
async function trackSockets(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as { WebSocket: typeof WebSocket; __sockets: WebSocket[]; __wsBlock: boolean };
    const Native = w.WebSocket;
    w.__sockets = [];
    w.__wsBlock = false;
    w.WebSocket = class extends Native {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(w.__wsBlock ? 'ws://127.0.0.1:9/' : url, protocols);
        w.__sockets.push(this);
      }
    };
  });
}

/** Cuts the connection; with `block`, reconnects fail until `unblock`. */
async function dropConnection(page: Page, block: boolean): Promise<void> {
  await page.evaluate((block) => {
    const w = window as unknown as { __sockets: WebSocket[]; __wsBlock: boolean };
    w.__wsBlock = block;
    for (const s of w.__sockets) if (s.readyState <= WebSocket.OPEN) s.close(3000, 'e2e drop');
  }, block);
}

interface Debug {
  map: string;
  loading: boolean;
  x: number;
  y: number;
  channel: string;
  offline: boolean;
  hp: number | null;
  maxHp: number | null;
}

async function debug(page: Page): Promise<Debug> {
  return page.evaluate(() => JSON.parse((window as unknown as { gameDebug: () => string }).gameDebug()) as Debug);
}

/** The stage the debug line names (`· Stage-7 ·`). */
async function stageName(page: Page): Promise<string> {
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  return /Stage-\d+/.exec((await page.locator('body').textContent()) ?? '')![0];
}

/** Connection errors of the sockets the test sent to a dead port. */
const unrelated = (e: string) => /WebSocket connection to 'ws:\/\/127\.0\.0\.1:9\//.test(e);

test('a dropped connection rejoins the mission; a reload offers to return to battle', async ({ page }, info) => {
  // The wave has to reach the hero first.
  test.setTimeout(180_000);
  const errors = watchErrors(page);
  await trackSockets(page);
  const user = await signUp(page, `/?mission=${MISSION}`);
  const hero = await createCharacter(page, 'Acolyte');
  await enterWorld(page);
  await page.getByRole('button', { name: 'Start' }).click();
  const stage = await stageName(page);
  await expect.poll(async () => (await debug(page)).loading).toBe(false);

  // Off the spawn, so a fresh arrival would show; then let the wave come
  // until it draws blood (an acolyte swings softly, so the porings get
  // close) (a fresh fighter would arrive at full health).
  await hold(page, 'KeyD', 700);
  await expect
    .poll(async () => {
      const d = await debug(page);
      return d.hp !== null && d.maxHp !== null && d.hp < d.maxHp;
    }, { timeout: 90_000, intervals: [500] })
    .toBe(true);
  const before = await debug(page);

  // The socket drops and the server stays out of reach for a while.
  await dropConnection(page, true);
  const plate = page.locator('.u-conn');
  await expect(plate).toBeVisible();
  await expect(plate).toContainText(/Connection lost — reconnecting… \(attempt \d+\)/);
  await expect.poll(async () => (await debug(page)).offline).toBe(true);
  // Input is frozen: nothing is predicted while nobody hears it.
  const frozen = await debug(page);
  await hold(page, 'KeyA', 500);
  expect((await debug(page)).x).toBe(frozen.x);
  await info.attach('reconnecting', { body: await page.screenshot(), contentType: 'image/png' });

  // Reachable again: back on the same stage, as the same fighter.
  await page.evaluate(() => ((window as unknown as { __wsBlock: boolean }).__wsBlock = false));
  await expect(plate).toBeHidden({ timeout: 30_000 });
  // The vitals of the re-attached fighter come right behind `stage_joined`
  // (read before anything on the field can heal it).
  await expect.poll(async () => (await debug(page)).hp, { intervals: [50] }).not.toBeNull();
  const rejoined = await debug(page);
  expect(await stageName(page)).toBe(stage);
  await expect.poll(async () => (await debug(page)).loading).toBe(false);
  const after = await debug(page);
  expect(after.offline).toBe(false);
  expect(rejoined.maxHp).toBe(before.maxHp);
  // Link-dead, the character kept its wounds (and maybe took more; a fresh
  // arrival would be at full health) and stood where it was.
  expect(rejoined.hp!).toBeLessThanOrEqual(before.hp!);
  expect(Math.abs(after.x - before.x)).toBeLessThan(1);
  expect(Math.abs(after.y - before.y)).toBeLessThan(1);

  // The page reloads mid-mission (a crash): signing in again offers the
  // mission the character is still in.
  await page.goto('/');
  await page.getByPlaceholder('username').fill(user);
  await page.getByPlaceholder('password').fill('e2e-password');
  await page.getByRole('button', { name: 'Log in' }).click();
  await expect(page.locator('.login')).toHaveCount(0);
  await page.locator('.u-cbox', { hasText: hero }).click();
  await page.getByRole('button', { name: /Enter world/ }).click();
  const offer = page.locator('.u-dialog', { hasText: 'Return to battle' });
  await expect(offer).toBeVisible();
  await expect(offer).toContainText(/still fighting/);
  await info.attach('return to battle', { body: await page.screenshot(), contentType: 'image/png' });
  await offer.getByRole('button', { name: 'Return to battle' }).click();
  expect(await stageName(page)).toBe(stage);
  await expect(page.locator('body')).not.toContainText(/· Hub-\d+ ·/);

  expect(errors.filter((e) => !unrelated(e))).toEqual([]);
});
