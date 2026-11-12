# ws_flutter

App lifecycle for the [`ws`](https://pub.dev/packages/ws) WebSocket client
in Flutter apps.

Mobile operating systems freeze an app soon after it leaves the screen and
may kill its sockets without telling either side. When the app comes back,
the connection looks open but is dead, and timers that should have noticed
fire all at once. `WebSocketLifecycle` handles this:

| Event | What happens |
|---|---|
| App in the background (`paused`, Android and iOS) | `client.suspend()`: no heartbeat, no reconnect attempts; an open connection stays open |
| App visible again after `paused` or `hidden` | `client.resume()`: one ping; a dead connection is replaced at once, without the reconnect backoff |
| Network changed, in the foreground | `client.resume()` |

```dart
import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:ws/ws.dart';
import 'package:ws_flutter/ws_flutter.dart';

final client = WebSocketClient.json();
final lifecycle = WebSocketLifecycle(
  client,
  // Optional: any stream whose events mean "the network changed".
  networkChanges: Connectivity().onConnectivityChanged,
);
await client.connect('wss://example.com/ws');

// When the client is no longer needed:
lifecycle.dispose();
await client.close();
```

`disconnectInBackground: true` also closes the connection when the app
goes to the background. The server is freed sooner, but every trip to
another app then costs a full reconnect.

Desktop and web never report `paused`, so a minimized window or a background
tab keeps its connection; it is only checked when the app becomes visible
again.

Calling `connect()` or `reconnect()` in the background ends the suspension,
for example when a push message wakes the app to sync.

## Keeping a socket alive in the background

The OS does not allow it. Use push messages (APNs, FCM) to wake the app,
then reconnect and fetch what was missed. See "Background and network
changes" in the `ws` README.
