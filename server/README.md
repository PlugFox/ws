# ws_bench_server

Rust WebSocket server (tokio + tungstenite + rustls) used by the `ws`
package for benchmarks, tests and examples.

```bash
cargo run --release -- [--host 127.0.0.1] [--port 9090] [--tls-port 9443]
# or from the repository root
make server
```

The TLS port uses a self-signed certificate for `localhost` / `127.0.0.1`
that is generated at startup, so clients must skip certificate verification.

## Endpoints

The same endpoints are available on both ports (`ws://` and `wss://`).

### `/echo`

Sends every text and binary message back unchanged.

### `/stream`

Pushes messages from the server to the client, then closes the connection
with code `1000` and reason `done`.

| query      | default | meaning                                                         |
|------------|---------|-----------------------------------------------------------------|
| `format`   | `json`  | `json`: text frames; `binary`: binary frames                     |
| `size`     | `1024`  | approximate message size in bytes                               |
| `rate`     | `1000`  | messages per second; `0` sends as fast as possible              |
| `duration` | `5000`  | stream length in ms; the message count is `rate * duration / 1000` |
| `count`    | —       | explicit message count; overrides `duration`                    |
| `unicode`  | `0`     | `1` puts non-ASCII strings into JSON payloads                   |

Every message carries a sequence number and the send time, in microseconds
since the Unix epoch:

- `json` messages look like `{"seq":N,"ts":T,"items":[{...}, ...]}`, where
  `items` holds realistic objects with strings, numbers, booleans, arrays
  and nested maps.
- `binary` messages start with `u64 LE seq` and `u64 LE ts`, followed by
  filler bytes.

Example:

```bash
websocat 'ws://127.0.0.1:9090/stream?size=200&rate=2&duration=3000'
```
