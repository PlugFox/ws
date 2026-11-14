import 'dart:convert';
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

final class ApiException implements Exception {
  ApiException(this.status, this.message, {this.code, this.field});
  final int status;
  final String message;

  /// The server's error `code` (`email_taken`, `USER_EXISTS`, ...).
  final String? code;

  /// The form field the error is about (`email`, `telegram`), if any.
  final String? field;
  @override
  String toString() => message;
}

/// The session cannot be renewed (refresh token expired, revoked or
/// reused): the player has to sign in again.
final class SessionExpired implements Exception {
  SessionExpired(this.message);
  final String message;
  @override
  String toString() => message;
}

/// Minimal REST client for auth and characters (same origin).
final class Api {
  Api(this.base);

  final String base;

  /// Access token (`Authorization: Bearer`), its expiry and the refresh
  /// token that renews both (`/api/auth/refresh` rotates it every time).
  String? token;
  String? _refreshToken;
  DateTime? _expiresAt;

  /// A refresh in flight: rotation makes the old refresh token a reuse, so
  /// two at once would revoke the session.
  Future<String>? _refreshing;

  /// Signs out locally (the menu's Log out, an expired session).
  void forget() {
    token = null;
    _refreshToken = null;
    _expiresAt = null;
  }

  /// A current access token: renewed when it expires within [margin], or
  /// always with [force] (the server refused the one we had). Throws
  /// [SessionExpired] when the session cannot be renewed; other failures
  /// (offline) throw as they are and leave the tokens alone.
  Future<String> freshToken({Duration margin = const Duration(minutes: 2), bool force = false}) {
    final t = token, exp = _expiresAt;
    if (t != null && !force && (exp == null || DateTime.now().add(margin).isBefore(exp))) return Future.value(t);
    return _refreshing ??= _renew().whenComplete(() => _refreshing = null);
  }

  Future<String> _renew() async {
    final refresh = _refreshToken;
    if (refresh == null) throw SessionExpired('Your session has ended. Please sign in again.');
    final Map<String, Object?> r;
    try {
      r = await _call('POST', '/api/auth/refresh', {'refresh_token': refresh});
    } on ApiException catch (e) {
      if (e.status == 401 || e.status == 403) {
        forget();
        throw SessionExpired('Your session has expired. Please sign in again.');
      }
      rethrow;
    }
    _adopt(r);
    return token!;
  }

  void _adopt(Map<String, Object?> r) {
    token = r['access_token']! as String;
    _refreshToken = r['refresh_token'] as String? ?? _refreshToken;
    final secs = (r['expires_in'] as num?)?.toInt();
    _expiresAt = secs == null ? null : DateTime.now().add(Duration(seconds: secs));
  }

  Future<Map<String, Object?>> _call(String method, String path, [Object? body]) async {
    final headers = web.Headers()..set('Content-Type', 'application/json');
    if (token != null) headers.set('Authorization', 'Bearer $token');
    final resp = await web.window
        .fetch(
          '$base$path'.toJS,
          web.RequestInit(method: method, headers: headers, body: body == null ? null : jsonEncode(body).toJS),
        )
        .toDart;
    final text = (await resp.text().toDart).toDart;
    final Object? data = text.isEmpty ? null : jsonDecode(text);
    if (!resp.ok) {
      final msg = data is Map ? (data['error'] ?? data['message'] ?? text).toString() : text;
      throw ApiException(
        resp.status,
        msg.isEmpty ? 'HTTP ${resp.status}' : msg,
        code: data is Map ? data['code'] as String? : null,
        field: data is Map ? data['field'] as String? : null,
      );
    }
    return data is Map<String, Object?> ? data : {'items': data};
  }

  /// Signs in, or with [register] creates the account first; [email] and
  /// [telegram] are its optional contacts ('' = not given; registration
  /// only).
  Future<void> login(String username, String password, {required bool register, String email = '', String telegram = ''}) async {
    final r = await _call('POST', register ? '/api/auth/register' : '/api/auth/login', {
      'username': username,
      'password': password,
      if (register && email.isNotEmpty) 'email': email,
      if (register && telegram.isNotEmpty) 'telegram': telegram,
    });
    _adopt(r);
  }

  /// `GET /api/auth/me`: `{user: {id, username, status, created_at,
  /// contacts}, session}`.
  Future<Map<String, Object?>> me() async {
    await freshToken();
    return _call('GET', '/api/auth/me');
  }

  /// `PATCH /api/auth/contacts`: sets both contacts ('' clears one);
  /// answers the stored `contacts`.
  Future<Map<String, Object?>> updateContacts({required String email, required String telegram}) async {
    await freshToken();
    final r = await _call('PATCH', '/api/auth/contacts', {'email': email, 'telegram': telegram});
    return (r['contacts'] as Map?)?.cast<String, Object?>() ?? const {};
  }

  Future<List<Map<String, Object?>>> characters() async {
    final r = await _call('GET', '/api/characters');
    final list = r['items'] ?? r['characters'] ?? const <Object?>[];
    return (list as List).cast<Map<String, Object?>>();
  }

  /// Appearance fields as in the REST API (`gender`, `hair_style`,
  /// `hair_color`, `body_color`).
  Future<Map<String, Object?>> createCharacter(String name, String archetype, {Map<String, int> look = const {}}) =>
      _call('POST', '/api/characters', {'name': name, 'archetype': archetype, ...look});

  Future<void> select(int id) => _call('POST', '/api/characters/$id/select');

  Future<Uint8List> bytes(String path) async {
    final resp = await web.window.fetch('$base$path'.toJS).toDart;
    if (!resp.ok) throw ApiException(resp.status, 'GET $path: ${resp.status}');
    return (await resp.arrayBuffer().toDart).toDart.asUint8List();
  }

  Future<Map<String, Object?>> json(String path) async {
    final resp = await web.window.fetch('$base$path'.toJS).toDart;
    if (!resp.ok) throw ApiException(resp.status, 'GET $path: ${resp.status}');
    return jsonDecode((await resp.text().toDart).toDart) as Map<String, Object?>;
  }
}
