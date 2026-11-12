# ws_bench_server

Rust WebSocket server (tokio + tungstenite + rustls) used by the `ws`
package for benchmarks, conformance/fault tests and examples. It reads the
HTTP request head itself, so it can answer HTTP faults, craft broken `101`s
and script raw frames; well-behaved endpoints hand the socket to tungstenite.

```bash
cargo run --release -- [--host 127.0.0.1] [--port 9090] [--tls-port 9443] \
  [--badhost-port 0] [--proxy-port 0] [--proxy-auth-port 0] [--no-ipv6] \
  [--tls-cert chain.pem --tls-key key.pem] [--ready-json] [--verbose]
# or from the repository root
make server
```

Without `--ready-json` it prints the listeners to stderr and runs until
ctrl-c/SIGTERM. `--tls-cert`/`--tls-key` make the `wss` port serve your
own chain instead of the generated one, for example one issued by a CA
installed on a test device (`wss_spki` is then empty).

## Test harness contract (`--ready-json`)

```bash
ws_bench_server --host 127.0.0.1 --port 0 --tls-port 0 --ready-json
```

Binds every listener (port `0` = ephemeral), then prints exactly one line to
stdout:

```json
{"ws":62406,"wss":62407,"wss_badhost":62408,"wss_expired":62412,"ws6":62411,"proxy":62409,"proxy_auth":62410,"ca_pem":"-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----\n","wss_spki":"3059..."}
```

| key | meaning |
|---|---|
| `ws` | plain endpoints, plus `GET /ca.pem`, `/log`, `/proxy-log` |
| `wss` | TLS, certificate for `localhost`, `127.0.0.1`, `::1` |
| `wss_badhost` | TLS, certificate for `wrong.invalid` only |
| `wss_expired` | TLS, certificate for `localhost` that expired in 2021 |
| `ws6` | plain endpoints on `[::1]`, or `null` without IPv6 |
| `proxy` | HTTP CONNECT proxy, no auth |
| `proxy_auth` | CONNECT proxy requiring `Proxy-Authorization: Basic dGVzdDpzZWNyZXQ=` (`test:secret`) |
| `ca_pem` | the CA that signed the TLS certificates (generated at startup) |
| `wss_spki` | hex DER SubjectPublicKeyInfo of the `wss` certificate (pin tests) |

The process exits when stdin reaches EOF (so a dead parent leaves no
orphan) or on ctrl-c/SIGTERM. Per-connection errors are not printed unless
`--verbose`. Both TLS ports serve every endpoint.

## Endpoints

All query values are decimal unless named `hex`. `id=<token>` on any
endpoint records what the client sent (see `/log`). `then=`/`end=`/`mode=`
take `fin` (shutdown(Write), then close), `rst` (SO_LINGER 0), `hold` (keep
TCP open, read and discard) or `wait` (wait for the client's close frame,
then FIN, then wait for the client's FIN).

Subprotocols: the `101` echoes `protocol=` from the query if given (even if
not offered; `protocol=` empty sends none), else the first offered one.
Extensions are never negotiated.

### Benchmarks

| endpoint | behaviour |
|---|---|
| `/echo` | sends every text/binary message back |
| `/stream?format=json\|binary&size=1024&rate=1000&duration=5000&count=N&unicode=0\|1` | server-to-client stream; each message carries `seq` and a send timestamp (µs since epoch). `json`: `{"seq":N,"ts":T,"items":[...]}`; `binary`: `u64 LE seq`, `u64 LE ts`, filler. `rate=0` floods. Count is `count` or `rate*duration/1000`. Ends with close `1000 done`. |

### Plain HTTP (any port)

| endpoint | behaviour |
|---|---|
| `/ca.pem` | CA certificate PEM |
| `/log?id=` | JSON of what connection `id` sent (`404` if unknown), see below |
| `/proxy-log` | `{"tunnels":["127.0.0.1:1234",...]}`: every CONNECT target, authorized or not |

### Handshake level

| endpoint | behaviour |
|---|---|
| `/status?code=200&body=&header=Name:Value` | reply `code` with the headers (repeatable) and body, `Connection: close` |
| `/redirect?code=302&to=/echo` | 3xx with `Location: to` |
| `/redirect-loop?n=3&code=302` | redirects to itself with `n-1`, at `0` to `/echo`; other query params are carried over |
| `/hang` | never responds; holds TCP until the client leaves |
| `/delay?ms=500&then=/echo` | sleeps, then handles the request as `then` (outer query params carried over) |
| `/eof-handshake?partial=0\|1` | closes after the request; `partial=1` first writes `HTTP/1.1 101 Swi` |
| `/junk` | writes `220 smtp.example ESMTP\r\n` and closes |
| `/big-head?kb=64` | `101` with an `X-Filler` header of `kb` KiB, then echo |
| `/bad-101?omit=accept,upgrade,connection,protocol&accept=wrong\|dup\|nonb64&upgrade=&connection=&protocol=&extensions=` | crafted `101` (unset parts correct), then echo |
| `/headers` | `101`, then one text message `{method,target,version,headers:[[k,v],...],peer}`, then echo |

Request heads over 16 KiB get `431`; a plain GET to a WebSocket path gets
`426`; unknown paths get `404` (or, for an upgrade request, `101` then close
`1008`).

### After the handshake

| endpoint | behaviour |
|---|---|
| `/close?code=&reason=&after_ms=0&after_msgs=0&then=wait` | echo until both `after_msgs` messages were echoed and `after_ms` passed, then send close (no `code` → empty payload), then `then` |
| `/close-empty` | `/close` with an empty close payload |
| `/drop?after_ms=0&after_msgs=0&mode=fin\|rst` | same trigger, then drop TCP without a close frame |
| `/ignore-close` | echo, never answer a close frame, keep TCP open |
| `/silent?read=1` | send nothing, never pong; `read=0` also stops reading (window fills) |
| `/slow-read?bps=4096&pause_ms=0` | stop reading for `pause_ms`, then read at most `bps` bytes/s (`0` = unlimited); echo nothing; total in `/log` `bytes` |
| `/flood?count=10000&size=16&kind=text\|binary&then=close\|stay` | `count` messages of `size` bytes (`a..z` repeated) at once; then close `1000 done`, or `stay` (echo) |
| `/ping?payload=<hex>&interval_ms=0&count=1` | sends pings one at a time; after each pong sends text `pong-ok` or `pong-mismatch`; then echo |
| `/pong?interval_ms=100` | unsolicited empty pongs; never answers pings; echoes messages |
| `/stall?size=1000&sent=500` | binary frame header for `size`, only `sent` payload bytes, then hold |
| `/fragment?size=1024&parts=4&kind=text\|binary&ping_between=0\|1&split_utf8=0\|1` | one message in `parts` fragments (optional ping `between` after each non-final one), then echo. Text is `a..z` repeated; `split_utf8=1` uses `€` repeated (size rounded up to a multiple of 3) with every cut inside a code point. Binary is `i % 256`. |
| `/raw?hex=&preset=&chunk=0&delay_ms=0&end=wait` | writes the preset, then `hex` bytes, in `chunk`-byte writes (`0` = one) `delay_ms` apart, then `end`. Unknown preset or bad hex → `400` before the upgrade. |

`/raw` presets (default `end=wait`, `eof-*` default `fin`):

| group | presets |
|---|---|
| header bits, opcodes | `rsv1`, `rsv2`, `rsv3` (text `hi` with that bit), `opcode3`, `opcodeB`, `masked` (masked text `hi`) |
| control frames | `ping-nofin`, `ping126` (126-byte ping) |
| fragmentation | `cont-first`, `text-mid-frag` (new text frame inside a fragmented one), `close-mid-frag` (valid: close `1000` after an unfinished text fragment) |
| close frames | `close1b` (1-byte payload), `close-code-<n>` (e.g. `close-code-1005`), `close-bad-utf8` (`1000` + reason `FF FE`) |
| UTF-8 | `utf8-bad` (`ab FF`), `utf8-trunc` (`ab E2 82`), `utf8-frag-bad` (`ab E2 82` + continuation `28`) |
| lengths | `len64-msb` (64-bit length with MSB set), `len64-max` (2^63-1), `len-nonmin` (5 bytes in a 16-bit length) |
| truncation, limits | `eof-in-header`, `eof-in-payload`, `eof-between-frags`, `zero-cont-x10000` (empty text in 10000 empty continuations + final) |

### `/log?id=`

```json
{"id":"t1","frames":[{"op":1,"fin":true,"len":5,"masked":true,"rsv":0}],"frames_dropped":0,
 "close":{"code":1000,"reason":"bye","reason_utf8":true,"len":5},"bytes":20,"tcp":"fin","done":true}
```

`frames` lists every frame the client sent after the head (first 100 000);
`close` is the first close frame (`code` is `null` for an empty payload) or
`null`; `bytes` counts bytes read after the head; `tcp` is `fin`, `rst` or
`open`; `done` turns true once the server finished with the connection, so
poll until it does. A new connection with the same `id` replaces the entry;
entries live for the process lifetime. On `/echo`-style endpoints with `id`
the server half-closes after the close handshake and waits (≤10 s) for the
client's FIN so `tcp` is meaningful.

### CONNECT proxies

`CONNECT host:port` → connects upstream, answers
`HTTP/1.1 200 Connection established`, then splices both directions.
Upstream failure → `502`. On `proxy_auth`, a missing or wrong
`Proxy-Authorization` gets `407` with `Proxy-Authenticate: Basic realm="ws"`
and close. Non-CONNECT methods get `405`.

## Library

`start(&Options) -> Server` binds everything on the current tokio runtime
and returns the ports and CA; `listen(host, port, tls_port)` and
`start_embedded`/`embedded_cpu_micros` remain for the benchmarks.
