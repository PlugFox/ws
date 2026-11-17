-- Optional account contacts (Phase 6.27): an email address and a Telegram
-- username a player may give at sign-up or later in Settings → Account.
-- They are for what comes later: password recovery and notifications (an
-- SMTP sender, a Telegram bot) and Google sign-in. Both are stored as
-- normalised by `auth::contacts` (email: domain lowercased; Telegram:
-- without `@` / `t.me/`), case kept for display.
--
-- Unique regardless of case, like account names, but only when set
-- (partial indexes: any number of accounts have none). Lookups compare
-- `lower(col) = lower($1)` to use them.
--
-- `*_verified_at` stay NULL until a verification flow exists (a mailed
-- link, the bot's `/start <token>`); changing a contact clears its mark.
-- Nothing derived is stored: "is it Google mail" is computed from the
-- domain (`auth::contacts::is_google_mail`) and only matters as a hint;
-- Google sign-in will trust the OAuth token's verified email instead.
ALTER TABLE users
    ADD COLUMN email                TEXT,
    ADD COLUMN email_verified_at    TIMESTAMPTZ,
    ADD COLUMN telegram             TEXT,
    ADD COLUMN telegram_verified_at TIMESTAMPTZ;
CREATE UNIQUE INDEX users_email_lower_key ON users (lower(email)) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX users_telegram_lower_key ON users (lower(telegram)) WHERE telegram IS NOT NULL;
