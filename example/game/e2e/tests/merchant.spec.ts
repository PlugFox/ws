import { expect, test, type Page } from '@playwright/test';
import { cooldown, createCharacter, enterWorld, hold, learn, signUp, slot, watchErrors } from './helpers';

const MISSION = 'poring_island_survival';

test('a merchant shouts the party up and hammers the wave down', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page, `/?mission=${MISSION}`);
  // The Merchant is playable from the creation screen (no padlock, Create on).
  await createCharacter(page, 'Merchant');
  await enterWorld(page);

  // Level 30 (GAME_DEV_LEVEL): the Alchemist's tier is open.
  await page.keyboard.press('KeyK');
  await expect(page.locator('.u-skills')).toBeVisible();
  await learn(page, 'Axe Mastery', 1);
  await learn(page, 'Hammer Fall', 2);
  await learn(page, 'Adrenaline Rush', 1);
  await learn(page, 'Loud Exclamation', 1);
  await slot(page, 'Hammer Fall', 0);
  await slot(page, 'Adrenaline Rush', 1);
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-skills')).toBeHidden();

  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);

  // Adrenaline Rush goes off at once; Hammer Fall waits for the horde to
  // come within 3 m.
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
  await cast('Digit2', 'Adrenaline Rush');
  await cast('Digit1', 'Hammer Fall');
  await page.waitForTimeout(800);
  await page.screenshot({ path: info.outputPath('hammer_fall.png') });
  expect(errors).toEqual([]);
});

/** Server message kinds this spec watches on the socket (protocol.rs `kind`). */
const COMPANIONS = 0xa3;
const COMBAT = 0x2b;

/** LEB128 varint, as `WireWriter::var` writes it. */
function varint(n: number): number[] {
  const out: number[] = [];
  do {
    let b = n & 0x7f;
    n >>>= 7;
    if (n) b |= 0x80;
    out.push(b);
  } while (n);
  return out;
}

function readVarint(b: Uint8Array, at: number): [number, number] {
  let n = 0;
  let shift = 0;
  for (;;) {
    const x = b[at++];
    n |= (x & 0x7f) << shift;
    if (!(x & 0x80)) return [n >>> 0, at];
    shift += 7;
  }
}

function contains(hay: Uint8Array, needle: number[]): boolean {
  outer: for (let i = 0; i + needle.length <= hay.length; i++) {
    for (let j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) continue outer;
    return true;
  }
  return false;
}

/**
 * The game socket's server frames, one message each: the COMPANIONS
 * records (0xA3) and the COMBAT batches (0x2B) for the checks below.
 */
function watchSocket(page: Page) {
  const seen = { companions: [] as Uint8Array[], combat: [] as Uint8Array[] };
  page.on('websocket', (ws) =>
    ws.on('framereceived', ({ payload }) => {
      if (typeof payload === 'string') return;
      const b = new Uint8Array(payload);
      if (b[0] === COMPANIONS) seen.companions.push(b);
      else if (b[0] === COMBAT) seen.combat.push(b);
    }),
  );
  return {
    seen,
    /** Entity ids of the companions in every 0xA3 record so far. */
    companionIds(): number[] {
      const ids: number[] = [];
      for (const b of seen.companions) {
        // `var n, n × { var entity_id, ... }`: the first record's id is enough
        // to follow it; later records repeat it.
        const [n, at] = readVarint(b, 1);
        if (n > 0) ids.push(readVarint(b, at)[0]);
      }
      return ids;
    },
    /** A COMBAT batch naming `entity` (its swings and the hits on it). */
    fought(entity: number): boolean {
      const v = varint(entity);
      return seen.combat.some((b) => contains(b, v));
    },
  };
}

/** Says `text` in chat (GAME_DEV hooks: `/give <item>`). */
async function say(page: Page, text: string) {
  await page.keyboard.press('Enter');
  const input = page.locator('.u-chat input');
  await expect(input).toBeVisible();
  await input.fill(text);
  await input.press('Enter');
}

/** A slot of the town skill bar (in town, the tree closed), by skill name. */
function townSlot(page: Page, name: string) {
  return page.locator(`.u-hotbar.town.on .u-hk[data-name="${name}"]`);
}

/**
 * The Alchemist's companions and trade skills end to end (COMBAT.md §3.6):
 * in Izlude the town skill bar (Pharmacy brews, Call Homunculus calls it and
 * it follows); then a mission: it fights, Summon Flora, Sphere Mine,
 * Mammonite, the cart rolling behind; the shop's Discount price.
 * Screenshots of each step for a look.
 */
test('an alchemist calls a homunculus, plants flora and haggles', async ({ page }, info) => {
  // Town, a mission and the shop: longer than the default.
  test.setTimeout(240_000);
  const errors = watchErrors(page);
  const socket = watchSocket(page);
  const user = await signUp(page, `/?mission=${MISSION}`);
  await createCharacter(page, 'Merchant');
  await enterWorld(page);
  // Two Red Herbs for the brew.
  await say(page, '/give red_herb');
  await say(page, '/give red_herb');

  await page.keyboard.press('KeyK');
  await expect(page.locator('.u-skills')).toBeVisible();
  // Pushcart opens Discount and draws the cart; Bioethics opens the
  // homunculus and the plants; Learning Potion opens the Pharmacy.
  await learn(page, 'Pushcart', 1);
  await learn(page, 'Discount', 5);
  await learn(page, 'Mammonite', 5);
  await learn(page, 'Bioethics', 1);
  await learn(page, 'Call Homunculus', 1);
  await learn(page, 'Summon Flora', 1);
  await learn(page, 'Sphere Mine', 1);
  await learn(page, 'Learning Potion', 1);
  await learn(page, 'Pharmacy', 1);
  await slot(page, 'Call Homunculus', 0);
  await slot(page, 'Summon Flora', 1);
  await slot(page, 'Mammonite', 2);
  await slot(page, 'Sphere Mine', 3);
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-skills')).toBeHidden();

  // In town the bar is the town skill bar: Pharmacy and Call Homunculus
  // (marked `town` in skills.toml), not the combat skills.
  await expect(townSlot(page, 'Pharmacy')).toBeVisible();
  await expect(townSlot(page, 'Call Homunculus')).toBeVisible();
  await expect(townSlot(page, 'Mammonite')).toHaveCount(0);
  // A few steps east: the Pushcart rolls behind (west of) the merchant.
  await hold(page, 'KeyD', 700);
  await page.waitForTimeout(500);
  await page.screenshot({ path: info.outputPath('town_bar.png') });

  // Pharmacy from the town bar: brew a Red Potion from the two herbs.
  await townSlot(page, 'Pharmacy').click();
  const book = page.locator('[data-win="recipes"]');
  await expect(book).toBeVisible();
  await expect(book).toContainText('Pharmacy');
  await book.locator('.row', { hasText: 'Red Potion' }).click();
  await book.getByRole('button', { name: 'Make' }).click();
  await expect(book).toContainText('Brewed: Red Potion');
  await page.screenshot({ path: info.outputPath('pharmacy.png') });
  await page.keyboard.press('Escape');
  await expect(book).toBeHidden();
  await page.keyboard.press('Alt+KeyE');
  const inv = page.locator('[data-win="inventory"]');
  await expect(inv.locator('img[data-icon="item:red_potion"]').first()).toBeVisible();
  await expect(inv.locator('img[data-icon="item:red_herb"]')).toHaveCount(0);
  await page.keyboard.press('Escape');

  // Call Homunculus by its town bar key: the type chooser; Lif is picked.
  const key = (await townSlot(page, 'Call Homunculus').locator('.key').textContent())?.trim() ?? '';
  // A tick apart from the Alt+E above (a modifier seen in the same tick
  // would make it Alt+digit).
  await page.waitForTimeout(200);
  await page.keyboard.press(`Digit${key}`);
  const chooser = page.locator('[data-win="hom_choose"]');
  await expect(chooser).toBeVisible();
  await expect(chooser.locator('.card img').first()).toBeVisible();
  await page.screenshot({ path: info.outputPath('chooser.png') });
  await chooser.locator('.card', { hasText: 'Lif' }).click();
  await expect(chooser).toBeHidden();

  // The plate, and the homunculus out in Izlude beside its owner (a
  // COMPANIONS record names it).
  const plate = page.locator('[data-win="homunculus"]');
  await expect(plate).toBeVisible({ timeout: 10_000 });
  await expect(plate).toContainText('Lif');
  await expect.poll(() => socket.companionIds().length, { timeout: 10_000 }).toBeGreaterThan(0);
  const townHom = socket.companionIds()[0];
  expect(townHom).toBeGreaterThanOrEqual(0x60_0000);
  await hold(page, 'KeyD', 700);
  await page.waitForTimeout(800);
  await page.screenshot({ path: info.outputPath('izlude_homunculus.png') });
  // Its Change button: the chooser to change the type (5,000 zeny: the
  // starting purse is short, every card is locked).
  await plate.getByRole('button', { name: 'Change' }).click();
  await expect(chooser).toBeVisible();
  await expect(chooser).toContainText('Change your homunculus');
  await expect(chooser).toContainText(/Needs 5000 zeny/);
  await expect(chooser.locator('.card.off')).toHaveCount(4);
  await page.keyboard.press('Escape');
  await expect(chooser).toBeHidden();

  // Into the wave: it comes along and fights (a COMBAT batch names it).
  const before = socket.seen.companions.length;
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(page.locator('body')).toContainText(/· Stage-\d+ ·/);
  await expect.poll(() => socket.seen.companions.length, { timeout: 15_000 }).toBeGreaterThan(before);
  const hom = socket.companionIds().at(-1)!;
  expect(hom).toBeGreaterThanOrEqual(0x60_0000);
  await page.waitForTimeout(1_500);
  await page.screenshot({ path: info.outputPath('homunculus.png') });
  await expect.poll(() => socket.fought(hom), { timeout: 60_000, intervals: [1_000] }).toBe(true);

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
  // Summon Flora: a plant record (class 1) joins the list.
  const records = socket.seen.companions.length;
  await cast('Digit2', 'Summon Flora');
  await expect.poll(() => socket.seen.companions.length, { timeout: 10_000 }).toBeGreaterThan(records);
  await cast('Digit4', 'Sphere Mine');
  // Mammonite: a swing that spends zeny (the starting purse covers it).
  await cast('Digit3', 'Mammonite');
  await page.waitForTimeout(800);
  await page.screenshot({ path: info.outputPath('companions.png') });
  // A walk: the cart rolls behind.
  await hold(page, 'KeyD', 900);
  await page.screenshot({ path: info.outputPath('cart.png') });

  // Back in town: it comes home with its owner (a new record in Izlude).
  const away = socket.seen.companions.length;
  await page.getByRole('button', { name: 'Return to town' }).click();
  await expect(page.locator('body')).toContainText(/· Hub-\d+ ·/, { timeout: 30_000 });
  await expect.poll(() => socket.seen.companions.length, { timeout: 15_000 }).toBeGreaterThan(away);
  await page.waitForTimeout(1_000);
  await page.screenshot({ path: info.outputPath('back_in_town.png') });

  // Discount 5: the Tool Dealer's Red Potion shows its list price struck out
  // (`?shop=` opens it from the spawn; GAME_DEV trades from anywhere; the
  // reload logs in again).
  await page.goto(`/?shop=tool_dealer`);
  await page.getByPlaceholder('username').fill(user);
  await page.getByPlaceholder('password').fill('e2e-password');
  await page.getByRole('button', { name: 'Log in' }).click();
  await enterWorld(page);
  const shop = page.locator('[data-win="shop"]');
  await expect(shop).toContainText('Tool Dealer');
  const potion = shop.locator('.list .row', { hasText: 'Red Potion' });
  await expect(potion.locator('s')).toBeVisible();
  await expect(potion.locator('.p')).toHaveAttribute('title', /Discount: \d+% off/);
  await page.screenshot({ path: info.outputPath('discount.png') });
  expect(errors).toEqual([]);
});

/**
 * The town skill bar for another class: a Swordsman of the Crusader tier
 * with Peco Peco Ride mounts from it in Izlude.
 */
test('a crusader mounts the peco peco from the town skill bar', async ({ page }, info) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Swordsman');
  await enterWorld(page);
  await page.keyboard.press('KeyK');
  await expect(page.locator('.u-skills')).toBeVisible();
  await learn(page, 'Provoke', 5);
  await learn(page, 'Endure', 1);
  await learn(page, 'Peco Peco Ride', 1);
  await page.keyboard.press('Escape');
  await expect(page.locator('.u-skills')).toBeHidden();
  await expect(townSlot(page, 'Peco Peco Ride')).toBeVisible();
  await expect(page.locator('.u-hotbar.town.on .u-hk:not(.off)')).toHaveCount(1);
  const key = (await townSlot(page, 'Peco Peco Ride').locator('.key').textContent())?.trim() ?? '';
  await page.keyboard.press(`Digit${key}`);
  await expect(page.locator('body')).toContainText('You ride your Peco Peco');
  await page.waitForTimeout(600);
  await page.screenshot({ path: info.outputPath('peco.png') });
  // Again: off.
  await townSlot(page, 'Peco Peco Ride').click();
  await expect(page.locator('body')).toContainText('You get off your Peco Peco');
  expect(errors).toEqual([]);
});
