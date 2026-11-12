// What the shared server tests need from the Rust test server, on every
// platform: the VM starts it directly (test_server.dart), the browser
// through a hybrid isolate (browser_server.dart). No dart:io here.

/// A running `ws_bench_server` (server/README.md).
abstract interface class ServerHandle {
  /// `ws://` URL of [path] on the plain port.
  String url(String path);

  /// What connection `id` sent (`/log?id=`), once the server is done
  /// with it; with `done: false`, as soon as the entry exists.
  Future<Map<String, Object?>> log(String id, {bool done = true});
}

/// What a transport can observe, for the tests whose expectations differ.
typedef ServerCapabilities = ({
  /// Protocol pings: otherwise heartbeats use an application ping.
  bool protocolPing,

  /// Protocol violations are reported with the close code the client sent
  /// (1002, 1007, 1009). Browsers report 1006 and hide the reason.
  bool protocolCloseCodes,
});
