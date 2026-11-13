import 'dart:convert';
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

final class ApiException implements Exception {
  ApiException(this.status, this.message);
  final int status;
  final String message;
  @override
  String toString() => message;
}

/// Minimal REST client for auth and characters (same origin).
final class Api {
  Api(this.base);

  final String base;
  String? token;

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
      throw ApiException(resp.status, msg.isEmpty ? 'HTTP ${resp.status}' : msg);
    }
    return data is Map<String, Object?> ? data : {'items': data};
  }

  Future<void> login(String username, String password, {required bool register}) async {
    final r = await _call('POST', register ? '/api/auth/register' : '/api/auth/login', {
      'username': username,
      'password': password,
    });
    token = r['access_token']! as String;
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
