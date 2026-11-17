import { test, expect } from '@playwright/test';
import { signUp, createCharacter, enterWorld, unique, watchErrors } from './helpers';

/** The optional account contacts: the "Create account" form (inline
 * errors, normalisation) and Settings → Account (view, edit, clear). */
test('sign-up with email and Telegram, then edit them in Settings → Account', async ({ page }) => {
  const errors = watchErrors(page);
  const tag = unique('c');
  const email = `${tag}@Example.COM`;

  // A typo is caught before any request, under its field; Back keeps the form's values.
  await page.goto('/');
  await page.getByPlaceholder('username').fill(unique('e2e_'));
  await page.getByPlaceholder('password').fill('e2e-password');
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.locator('.login.create')).toContainText('Optional. Used later to recover your password');
  await expect(page.getByLabel('Email (optional)')).toHaveAttribute('autocomplete', 'email');
  await expect(page.getByLabel('Email (optional)')).toHaveAttribute('inputmode', 'email');
  await page.getByLabel('Telegram (optional)').fill('@abc');
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.locator('.login .ferr:not(:empty)')).toContainText('Telegram username has 5 to 32');
  await expect(page.getByLabel('Telegram (optional)')).toHaveAttribute('aria-invalid', 'true');
  await page.getByRole('button', { name: 'Back' }).click();
  await expect(page.getByRole('button', { name: 'Log in' })).toBeVisible();

  // The real sign-up: the server stores them normalised.
  const user = await signUp(page, '/', { email, telegram: `https://t.me/${tag}_tg` });
  // Another account holds an address (taken in any case).
  const other = await page.request.post('/api/auth/register', { data: { username: unique('e2e_'), password: 'e2e-password', email: `${tag}@taken.example` } });
  expect(other.status()).toBe(201);
  await createCharacter(page, 'Mage');
  await enterWorld(page);

  // Settings → Account shows them (domain lowercased, link stripped).
  await page.keyboard.press('Escape');
  await page.locator('.u-menu button', { hasText: 'Settings' }).click();
  await page.locator('.u-settings button', { hasText: 'Edit…' }).click();
  const account = page.locator('.u-account');
  await expect(account).toBeVisible();
  await expect(account.getByLabel('Email (optional)')).toHaveValue(`${tag}@example.com`);
  await expect(account.getByLabel('Telegram (optional)')).toHaveValue(`${tag}_tg`);
  await expect(account).toContainText('not verified');

  // A server-side refusal comes back inline; then clear Telegram and save.
  await account.getByLabel('Email (optional)').fill(`${tag.toUpperCase()}@TAKEN.example`);
  await account.getByRole('button', { name: 'Save' }).click();
  await expect(account.locator('.ferr:not(:empty)')).toContainText('already used by another account');
  await account.getByLabel('Email (optional)').fill(`${tag}@other.example`);
  await account.getByLabel('Telegram (optional)').fill('');
  await account.getByRole('button', { name: 'Save' }).click();
  await expect(account).toContainText('Saved.');
  await expect(account.getByLabel('Telegram (optional)')).toHaveValue('');

  // The taken address was a 409 the browser logs; nothing else.
  expect(errors.filter((e) => !e.includes('409 (Conflict)'))).toEqual([]);

  // The server agrees (a fresh login: it displaces the tab, so last).
  const login = await page.request.post('/api/auth/login', { data: { username: user, password: 'e2e-password' } });
  const token = (await login.json()).access_token as string;
  const me = await (await page.request.get('/api/auth/me', { headers: { Authorization: `Bearer ${token}` } })).json();
  expect(me.user.contacts).toEqual({ email: `${tag}@other.example`, email_verified: false, email_google: false, telegram: null, telegram_verified: false });
});
