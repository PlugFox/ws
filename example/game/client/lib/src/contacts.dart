/// The optional account contacts (an email address, a Telegram username):
/// the quick checks the "Create account" form and Settings → Account run
/// before a request, so obvious typos get an inline error at once. The
/// server (`auth::contacts`) is the authority: it checks again, normalises
/// (lowercase email domain, no `@` / `t.me/` on Telegram) and answers
/// `email_invalid`, `email_taken`, `telegram_invalid`, `telegram_taken`
/// with the `field`, which the forms show the same way.
library;

import 'dart:convert';

/// A problem with one field: which (`email`, `telegram`) and why.
typedef ContactProblem = ({String field, String message});

/// Longest address SMTP carries (server `contacts::EMAIL_MAX`).
const emailMax = 254;

final _email = RegExp(r"^[A-Za-z0-9!#$%&'*+/=?^_`{|}~-]+(\.[A-Za-z0-9!#$%&'*+/=?^_`{|}~-]+)*@([A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?\.)+[A-Za-z0-9-]*[A-Za-z][A-Za-z0-9-]*$");
final _telegram = RegExp(r'^[A-Za-z][A-Za-z0-9_]{4,31}$');
final _telegramPrefix = RegExp(r'^(https?://)?(www\.)?(t|telegram)\.me/', caseSensitive: false);

/// The Telegram username as the server will store it: without a link
/// prefix (`t.me/`, `https://t.me/`) or a leading `@`.
String telegramName(String raw) {
  var s = raw.trim().replaceFirst(_telegramPrefix, '');
  if (s.startsWith('@')) s = s.substring(1);
  while (s.endsWith('/')) {
    s = s.substring(0, s.length - 1);
  }
  return s;
}

/// The first problem with [email] / [telegram] (empty = not given), or
/// null when both look fine.
ContactProblem? contactProblem({String email = '', String telegram = ''}) {
  final e = email.trim();
  if (e.isNotEmpty) {
    if (e.length > emailMax) return (field: 'email', message: 'That email address is too long.');
    final at = e.lastIndexOf('@');
    if (at <= 0 || at > 64 || !_email.hasMatch(e)) {
      return (field: 'email', message: 'An email address looks like name@example.com.');
    }
  }
  final t = telegramName(telegram);
  if (t.isNotEmpty && !_telegram.hasMatch(t)) {
    return (field: 'telegram', message: 'A Telegram username has 5 to 32 letters, digits or _ and starts with a letter.');
  }
  return null;
}

/// `AccountSpec` fields (engine `account.ts`) from the `contacts` object
/// of `GET /api/auth/me` or `PATCH /api/auth/contacts`.
Map<String, Object?> accountSpec(String username, Map<String, Object?>? contacts) => {
  'username': username,
  'email': contacts?['email'],
  'emailVerified': contacts?['email_verified'] == true,
  'emailGoogle': contacts?['email_google'] == true,
  'telegram': contacts?['telegram'],
  'telegramVerified': contacts?['telegram_verified'] == true,
};

/// Settings → Account while open: what the engine window shows
/// (`showAccount`). The engine keeps what was typed across re-renders.
final class AccountForm {
  AccountForm(this.username, this.contacts);

  final String username;

  /// As the server stored them (`contacts` of `/me` or the update).
  Map<String, Object?> contacts;
  bool busy = false;
  String? error, field, note;

  /// Checks a save (`account_save {email, telegram}`): the values to send,
  /// or null with the problem shown.
  ({String email, String telegram})? submit(Map<String, Object?> e) {
    final email = '${e['email'] ?? ''}'.trim(), telegram = '${e['telegram'] ?? ''}'.trim();
    note = null;
    final p = contactProblem(email: email, telegram: telegram);
    if (p != null) {
      error = p.message;
      field = p.field;
      return null;
    }
    busy = true;
    error = field = null;
    return (email: email, telegram: telegram);
  }

  /// The server stored them.
  void saved(Map<String, Object?> stored) {
    contacts = stored;
    busy = false;
    error = field = null;
    note = 'Saved.';
  }

  /// The server refused (`field` from its reply, if any).
  void refused(String message, String? field) {
    busy = false;
    error = message;
    this.field = field;
  }

  /// `AccountSpec` JSON.
  String spec() => jsonEncode({
    ...accountSpec(username, contacts),
    'busy': busy,
    'error': ?error,
    'field': ?field,
    'note': ?note,
  });
}
