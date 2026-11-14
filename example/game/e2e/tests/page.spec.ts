import { expect, test, type Page } from '@playwright/test';
import { createCharacter, enterWorld, signUp, watchErrors } from './helpers';

/** Dispatches a cancelable event on the first match of `selector`; true when the page cancelled it. */
async function cancelled(page: Page, selector: string, make: string): Promise<boolean> {
  return page.evaluate(
    ([sel, src]) => {
      const target = sel === 'window' ? window : document.querySelector(sel)!;
      const e = new Function(`return ${src}`)() as Event;
      target.dispatchEvent(e);
      return e.defaultPrevented;
    },
    [selector, make],
  );
}

test('the page keeps the browser out of the game, text fields keep theirs', async ({ page }) => {
  const errors = watchErrors(page);
  await signUp(page);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  // No context menu, zoom shortcuts, selection or drag over the game.
  const ctx = `new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 300, clientY: 300 })`;
  expect(await cancelled(page, 'canvas', ctx)).toBe(true);
  expect(await cancelled(page, 'body', ctx)).toBe(true);
  expect(await cancelled(page, 'window', `new WheelEvent('wheel', { bubbles: true, cancelable: true, ctrlKey: true, deltaY: -100 })`)).toBe(true);
  expect(await cancelled(page, 'window', `new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: -100 })`)).toBe(false);
  expect(await cancelled(page, 'window', `new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ctrlKey: true, code: 'Equal' })`)).toBe(true);
  expect(await cancelled(page, 'window', `new KeyboardEvent('keydown', { bubbles: true, cancelable: true, metaKey: true, code: 'Minus' })`)).toBe(true);
  expect(await cancelled(page, 'body', `new Event('selectstart', { bubbles: true, cancelable: true })`)).toBe(true);
  expect(await page.evaluate(() => getComputedStyle(document.body).userSelect)).toBe('none');
  expect(await page.evaluate(() => getComputedStyle(document.documentElement).touchAction)).toBe('none');

  // Alt+H: the Social window on its Friends tab, with the name field.
  await page.keyboard.press('Alt+KeyH');
  const social = page.locator('[data-win="social"]');
  await expect(social.locator('.u-tabs button.on')).toHaveText(/^Friends/);
  const field = social.locator('input');
  await expect(field).toBeVisible();
  // The field keeps its own menu and selection.
  expect(await cancelled(page, '[data-win="social"] input', ctx)).toBe(false);
  expect(await cancelled(page, '[data-win="social"] input', `new Event('selectstart', { bubbles: true, cancelable: true })`)).toBe(false);
  expect(await field.evaluate((el) => getComputedStyle(el).userSelect)).toBe('text');
  await field.fill('Copy me');
  expect(await field.evaluate((el: HTMLInputElement) => (el.select(), el.selectionEnd! - el.selectionStart!))).toBe(7);

  // Alt+Z: the same window, now on the Party tab (not in a party yet);
  // pressing it again closes the window.
  await field.blur();
  await page.keyboard.press('Alt+KeyZ');
  await expect(social.locator('.u-tabs button.on')).toHaveText(/^Party/);
  await expect(social.getByRole('button', { name: 'Find a party' })).toBeVisible();
  await page.keyboard.press('Alt+KeyZ');
  await expect(social).toHaveCount(0);
  expect(errors).toEqual([]);
});
