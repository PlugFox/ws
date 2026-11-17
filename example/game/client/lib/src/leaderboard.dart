/// The leaderboard's pages (`GET /api/leaderboard`) for the engine's Ranking
/// tab: the tab asks (`leaderboard_query {sort, page, limit}`), this fetches
/// and hands the server's answer to `GameView.setLeaderboard`, with the
/// player's own row (`me`) and the online count.
library;

import 'dart:convert';

/// Boards the server ranks (`sort`).
const leaderboardSorts = {'level', 'kills', 'mvp', 'zeny'};

/// The server's page size cap.
const leaderboardMaxLimit = 50;

/// The request for a page of [sort]; [me] adds the player's own row.
String leaderboardPath(String sort, int page, int limit, {int? me}) =>
    '/api/leaderboard?sort=$sort&page=$page&limit=$limit${me != null ? '&me=$me' : ''}';

final class LeaderboardClient {
  LeaderboardClient(this._get, this._show, {required this.me, required this.online});

  /// GET a path, decoded.
  final Future<Map<String, Object?>> Function(String path) _get;

  /// `GameView.setLeaderboard`.
  final void Function(String json) _show;

  /// The player's character id (null before the world says).
  final int? Function() me;

  /// Players online, when known.
  final int? Function() online;

  /// The latest query: an answer to an older one is dropped.
  int _seq = 0;

  /// Answers a `leaderboard_query` event.
  Future<void> query(Map<String, Object?> e) async {
    final sort = leaderboardSorts.contains(e['sort']) ? e['sort']! as String : 'level';
    final page = ((e['page'] as num?)?.toInt() ?? 1).clamp(1, 1 << 20);
    final limit = ((e['limit'] as num?)?.toInt() ?? 10).clamp(1, leaderboardMaxLimit);
    final seq = ++_seq;
    Map<String, Object?> spec;
    try {
      spec = await _get(leaderboardPath(sort, page, limit, me: me()));
    } on Object {
      spec = {
        'sort': sort,
        'page': page,
        'pages': 1,
        'total': 0,
        'entries': const <Object?>[],
        'error': 'The rankings are unavailable right now.',
      };
    }
    if (seq != _seq) return;
    final n = online();
    _show(jsonEncode({...spec, 'online': ?n}));
  }
}
