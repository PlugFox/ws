# Web Socket Echo Server

## How to connect to the server using Node.js

```bash
$ dart run server/bin/server.dart
$ npm install -g wscat
$ wscat -c ws://127.0.0.1:8080/connect
Connected (press CTRL+C to quit)
> hello
< hello
```

## How to connect to the server using Python

```bash
$ dart run server/bin/server.dart
$ python -m pip install websockets
$ python -m websockets ws://127.0.0.1:8080/connect
Connected to ws://127.0.0.1:8080/connect.
> hello
< hello
```

## How to connect to the server using Dart

```bash
$ dart run server/bin/server.dart
$ dart run --define=URL=ws://127.0.0.1:8080/connect example/ws_example.dart
* WebSocketClientState.connecting(ws://127.0.0.1:8080/connect)
* WebSocketClientState.open(ws://127.0.0.1:8080/connect)
< Hello,
< world!
* WebSocketClientState.disconnecting(NORMAL_CLOSURE)
* WebSocketClientState.closed(NORMAL_CLOSURE)
Metrics:
- readyState: CLOSED
- reconnectTimeout: 5 seconds
- transferredSize: 13
- receivedSize: 13
- transferredCount: 2
- receivedCount: 2
- reconnects: 1 / 1
- lastSuccessfulConnectionTime: 1 seconds ago
- disconnects: 1
- lastDisconnectTime: 1 seconds ago
- expectedReconnectTime: never
- lastDisconnect: 1000 (NORMAL_CLOSURE)
- lastUrl: ws://127.0.0.1:8080/connect
```
