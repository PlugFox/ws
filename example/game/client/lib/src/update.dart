/// The game is updated under an open tab: this client's build, sent on
/// connect (`?build=` on `/api/ws`), and what to do when the server takes
/// another one (`server/src/version.rs`):
/// - refused on connect (close [closeUpdateRequired]): reload at once,
///   once per server build; a reload that meets the refusal again (a stale
///   cache, a rollout half done) shows the "Update available — Reload"
///   modal instead of looping;
/// - a status poll (`GET /api/online`, `build`) names another build while
///   this tab plays: a small "Update available" button, no interruption.
library;

/// This client's build, stamped by `tool/build.sh` (`-DGAME_BUILD`): the
/// commit's short hash, as the server's `build.rs` stamps it; `dev` when
/// built without git (and in `dart test`).
const clientBuildId = String.fromEnvironment('GAME_BUILD', defaultValue: 'dev');

/// WebSocket close code: the server refused this build.
const closeUpdateRequired = 4010;

/// The server's build named by an update close (`update_required:<build>`;
/// '' when the reason names none), or null when [code] is not one.
String? refusedBy(int? code, String? reason) {
  if (code != closeUpdateRequired) return null;
  const prefix = 'update_required:';
  final r = reason ?? '';
  return r.startsWith(prefix) ? r.substring(prefix.length) : '';
}

/// Whether a server taking [serverBuild] runs another build than [client].
/// An unknown build (an older server, a failed poll) is not another.
bool isOtherBuild(String? serverBuild, {String client = clientBuildId}) =>
    serverBuild != null && serverBuild.isNotEmpty && serverBuild != client;

/// The page's update state; the engine's `setUpdate` spec goes to [show]
/// (null hides), [reload] reloads the page, [read] / [write] keep the
/// auto-reload marker for the tab (`sessionStorage`; [write] says whether
/// it was kept: without it no automatic reload, which could loop).
final class UpdateWatch {
  UpdateWatch({
    required this.show,
    required this.reload,
    required this.read,
    required this.write,
    this.client = clientBuildId,
  });

  final void Function(Map<String, Object?>? spec) show;
  final void Function() reload;
  final String? Function(String key) read;
  final bool Function(String key, String value) write;
  final String client;

  /// The server build this tab last reloaded itself for.
  static const reloadedKey = 'ge.update.reloaded';

  /// The server refused this build: nothing plays until a reload.
  bool get required => _required;
  bool _required = false;

  /// The soft button is up.
  bool get offered => _offered;
  bool _offered = false;

  /// The server refused this build; [serverBuild] is the one it takes
  /// ('' when it did not say). Reloads at once the first time for that
  /// build, else shows the modal and leaves the reload to the player.
  void refused(String serverBuild) {
    _required = true;
    final key = serverBuild.isEmpty ? '?' : serverBuild;
    if (read(reloadedKey) != key && write(reloadedKey, key)) {
      show({'required': true, 'text': 'A new version of the game is out. Reloading…'});
      reload();
      return;
    }
    show({'required': true});
  }

  /// A status poll named [serverBuild]: offers a reload (once) when it is
  /// another build.
  void sawServer(String? serverBuild) {
    if (_required || _offered || !isOtherBuild(serverBuild, client: client)) return;
    _offered = true;
    show(const {});
  }
}

/// The page's [UpdateWatch] (`web/main.dart` sets it).
UpdateWatch? pageUpdates;
