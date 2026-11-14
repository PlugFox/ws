import 'dart:convert';

import 'package:game_client/src/contacts.dart';
import 'package:test/test.dart';

void main() {
  group('contactProblem', () {
    test('both optional', () {
      expect(contactProblem(), isNull);
      expect(contactProblem(email: '  ', telegram: ' @ '), isNull);
    });

    test('emails', () {
      for (final ok in ['ann@example.com', ' Ann.Lee+game@GMail.COM ', "o'neil@sub-d.example.org", 'x@mail.xn--p1ai']) {
        expect(contactProblem(email: ok), isNull, reason: ok);
      }
      for (final bad in ['plain', '@example.com', 'ann@', 'ann@example', 'a..b@example.com', '.a@example.com', 'ann@-x.com', 'an n@x.com', 'ann@1.2.3.4']) {
        expect(contactProblem(email: bad)?.field, 'email', reason: bad);
      }
      expect(contactProblem(email: '${'a' * 65}@example.com')?.field, 'email');
      expect(contactProblem(email: '${'a' * 60}@${'b' * 200}.com')?.field, 'email', reason: 'over 254');
    });

    test('telegram names and links', () {
      for (final raw in ['fox_dev', '@fox_dev', 't.me/fox_dev', 'https://t.me/fox_dev/', 'HTTPS://Telegram.me/fox_dev']) {
        expect(telegramName(raw), 'fox_dev', reason: raw);
        expect(contactProblem(telegram: raw), isNull, reason: raw);
      }
      for (final bad in ['abcd', '1abcde', '_abcde', 'abc-de', 'abcdé', 'a' * 33]) {
        expect(contactProblem(telegram: bad)?.field, 'telegram', reason: bad);
      }
    });

    test('the email is checked first', () {
      expect(contactProblem(email: 'x', telegram: 'y')?.field, 'email');
    });
  });

  group('AccountForm', () {
    Map<String, Object?> spec(AccountForm f) => jsonDecode(f.spec()) as Map<String, Object?>;

    test('shows the stored contacts, refuses typos locally, then saves', () {
      final f = AccountForm('ann', {'email': 'ann@gmail.com', 'email_verified': false, 'email_google': true, 'telegram': null});
      expect(spec(f), containsPair('email', 'ann@gmail.com'));
      expect(spec(f), containsPair('emailGoogle', true));
      expect(spec(f), containsPair('telegram', null));

      expect(f.submit({'email': 'nope', 'telegram': ''}), isNull);
      expect(spec(f), containsPair('field', 'email'));
      expect(f.busy, isFalse);

      final send = f.submit({'email': ' ann@work.example ', 'telegram': '@ann_lee'});
      expect(send, (email: 'ann@work.example', telegram: '@ann_lee'));
      expect(spec(f), containsPair('busy', true));
      expect(spec(f).containsKey('error'), isFalse);

      f.saved({'email': 'ann@work.example', 'telegram': 'ann_lee'});
      expect(spec(f), allOf(containsPair('telegram', 'ann_lee'), containsPair('note', 'Saved.'), containsPair('busy', false)));
    });

    test('a server refusal keeps its field', () {
      final f = AccountForm('ann', const {})..submit({'email': 'a@b.co', 'telegram': ''});
      f.refused('That email address is already used by another account.', 'email');
      expect(spec(f), allOf(containsPair('field', 'email'), containsPair('busy', false)));
      expect(spec(f).containsKey('note'), isFalse);
    });
  });
}
