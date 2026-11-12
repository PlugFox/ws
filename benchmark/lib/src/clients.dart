import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'codec.dart';
import 'model.dart';
import 'library.dart';
import 'native.dart';
import 'tape.dart';

/// Benchmarked client implementations.
enum Variant {
  /// `dart:io` WebSocket on the main isolate, `jsonDecode(String)` on main.
  a('a', 'dart:io WebSocket, main isolate'),

  /// `dart:io` WebSocket in a worker isolate, JSON decoded in the worker,
  /// decoded objects posted to main.
  b('b', 'dart:io WebSocket, worker isolate + decode'),

  /// Own codec on the main isolate, fused UTF-8 + JSON decoder on bytes.
  c('c', 'own codec, main isolate'),

  /// Own codec in a worker isolate, JSON decoded in the worker,
  /// decoded objects posted to main (batched per socket read).
  cWorker('c-worker', 'own codec, worker isolate + decode'),

  /// Own codec in a worker isolate, raw bytes posted to main as
  /// `TransferableTypedData`, decoded on main. Models a native (Rust)
  /// transport that delivers bytes and leaves decoding to Dart.
  cWorkerRaw('c-worker-raw', 'own codec, worker isolate, decode on main'),

  /// Own codec on main, JSON parsed into a tape and read through lazy
  /// `TapeMap` / `TapeList` views.
  cTape('c-tape', 'own codec, main isolate, tape + lazy views'),

  /// Own codec in a worker isolate that parses JSON into a tape; the tape is
  /// moved to main in O(1) and read through lazy views.
  cWorkerTape('c-worker-tape', 'own codec, worker isolate builds tape, lazy views on main'),

  /// Own codec in a worker isolate that decodes JSON *and* maps it to typed
  /// models (`Order.fromJson`); models are posted to main. Models a
  /// user-supplied decoder running off the main isolate.
  cWorkerModel('c-worker-model', 'own codec, worker isolate decodes to models'),

  /// Rust (tokio) transport, payload bytes as external typed data,
  /// JSON decoded on main.
  dBytes('d-bytes', 'Rust transport, bytes to Dart, decode on main'),

  /// Rust transport, simd-json → tape, lazy views on main.
  dTape('d-tape', 'Rust transport + simd-json tape, lazy views on main'),

  /// Rust transport, simd-json → Dart_CObject tree rebuilt by the VM on main.
  dCobject('d-cobject', 'Rust transport + simd-json, Dart_CObject tree'),

  /// The library (`package:ws` 2.0) through its public API, pure-Dart
  /// transport.
  wsDart('ws-dart', 'package:ws WebSocketClient, pure-Dart transport'),

  /// The library through its public API with `ws_native` installed.
  wsNative('ws-native', 'package:ws WebSocketClient, ws_native transport');

  const Variant(this.id, this.description);

  final String id;
  final String description;

  static Variant parse(String id) =>
      values.firstWhere((v) => v.id == id, orElse: () => throw ArgumentError(id));
}

/// Fused UTF-8 → JSON decoder: on the VM this parses bytes directly
/// without materializing an intermediate `String`.
final Converter<List<int>, Object?> _jsonUtf8 = utf8.decoder.fuse(json.decoder);

Object? decodeJsonBytes(Uint8List bytes) => _jsonUtf8.convert(bytes);

Uint8List encodeJsonBytes(Object? value) {
  final bytes = JsonUtf8Encoder().convert(value);
  return bytes is Uint8List ? bytes : Uint8List.fromList(bytes);
}

/// A connected benchmark client. Messages are delivered on the main isolate
/// as `Map<String, Object?>` (JSON text) or `Uint8List` (binary).
abstract interface class BenchClient {
  static Future<BenchClient> connect(
    Variant variant,
    Uri uri, {
    required void Function(Object message) onMessage,
    String format = 'json',
  }) =>
      switch (variant) {
        Variant.wsDart => LibraryClient.connect(uri, onMessage, format, native: false),
        Variant.wsNative => LibraryClient.connect(uri, onMessage, format, native: true),
        Variant.a => _IoClient.connect(uri, onMessage),
        Variant.c => _CodecClient.connect(uri, onMessage),
        Variant.cTape => _CodecClient.connect(uri, onMessage, tape: true),
        Variant.dBytes => NativeClient.connect(uri, NativeMode.bytes, onMessage),
        Variant.dTape => NativeClient.connect(uri, NativeMode.tape, onMessage),
        Variant.dCobject => NativeClient.connect(uri, NativeMode.cobject, onMessage),
        Variant.b ||
        Variant.cWorker ||
        Variant.cWorkerRaw ||
        Variant.cWorkerTape ||
        Variant.cWorkerModel =>
          _WorkerClient.connect(variant, uri, onMessage),
      };

  /// Sends a JSON map as a text message or a `Uint8List` as binary.
  void send(Object message);

  Future<void> get done;

  int? get closeCode;

  Future<void> close();
}

HttpClient _insecureHttpClient() =>
    HttpClient()..badCertificateCallback = (_, _, _) => true;

// --- A: dart:io on main -----------------------------------------------------

final class _IoClient implements BenchClient {
  _IoClient(this._ws, void Function(Object) onMessage) {
    _ws.listen(
      (Object? data) => switch (data) {
        String text => onMessage(jsonDecode(text) as Object),
        Uint8List bytes => onMessage(bytes),
        _ => throw StateError('Unexpected ${data.runtimeType}'),
      },
      onDone: () => _done.complete(),
      onError: (Object _) {},
    );
  }

  static Future<BenchClient> connect(Uri uri, void Function(Object) onMessage) async {
    final ws = await WebSocket.connect(
      uri.toString(),
      compression: CompressionOptions.compressionOff,
      customClient: _insecureHttpClient(),
    );
    return _IoClient(ws, onMessage);
  }

  final WebSocket _ws;
  final Completer<void> _done = Completer<void>();

  @override
  void send(Object message) => switch (message) {
        Uint8List bytes => _ws.add(bytes),
        _ => _ws.add(jsonEncode(message)),
      };

  @override
  Future<void> get done => _done.future;

  @override
  int? get closeCode => _ws.closeCode;

  @override
  Future<void> close() async {
    await _ws.close(1000);
    await done;
  }
}

// --- C: own codec on main ---------------------------------------------------

final class _CodecClient implements BenchClient {
  _CodecClient._();

  static Future<BenchClient> connect(
    Uri uri,
    void Function(Object) onMessage, {
    bool tape = false,
  }) async {
    final client = _CodecClient._();
    client._ws = await WsCodecClient.connect(
      uri,
      allowBadCertificates: true,
      onMessage: (opcode, payload) => opcode == opText
          ? onMessage(tape
              ? TapeDocument(buildTape(payload)).root!
              : decodeJsonBytes(payload)!)
          : onMessage(payload),
    );
    return client;
  }

  late final WsCodecClient _ws;

  @override
  void send(Object message) => switch (message) {
        Uint8List bytes => _ws.sendBinary(bytes),
        _ => _ws.sendUtf8(encodeJsonBytes(message)),
      };

  @override
  Future<void> get done => _ws.done;

  @override
  int? get closeCode => _ws.closeCode;

  @override
  Future<void> close() => _ws.close();
}

// --- Worker isolate variants ------------------------------------------------

final class _WorkerClient implements BenchClient {
  _WorkerClient._();

  late final SendPort _commands;
  final Completer<void> _done = Completer<void>();
  int? _closeCode;

  static Future<BenchClient> connect(
    Variant variant,
    Uri uri,
    void Function(Object) onMessage,
  ) async {
    final client = _WorkerClient._();
    final port = RawReceivePort();
    final opened = Completer<void>();
    port.handler = (Object? message) {
      switch (message) {
        case List<Object?> batch:
          for (final item in batch) {
            switch (item) {
              // JSON tape built by the worker: lazy views on main.
              case (tape: TransferableTypedData tape):
                onMessage(TapeDocument(tape.materialize().asUint8List()).root!);
              // Raw UTF-8 text: decode on main.
              case (TransferableTypedData text,):
                onMessage(decodeJsonBytes(text.materialize().asUint8List())!);
              case TransferableTypedData binary:
                onMessage(binary.materialize().asUint8List());
              case Object decoded:
                onMessage(decoded);
              case null:
                break;
            }
          }
        case SendPort commands:
          client._commands = commands;
        case ('open',):
          opened.complete();
        case ('error', String error):
          if (!opened.isCompleted) opened.completeError(WebSocketException(error));
        case ('closed', int? code):
          client._closeCode = code;
          port.close();
          client._done.complete();
      }
    };
    await Isolate.spawn(_workerMain, (port.sendPort, variant.id, uri.toString()));
    await opened.future;
    return client;
  }

  @override
  void send(Object message) => _commands.send(
        message is Uint8List ? TransferableTypedData.fromList([message]) : message,
      );

  @override
  Future<void> get done => _done.future;

  @override
  int? get closeCode => _closeCode;

  @override
  Future<void> close() {
    _commands.send('close');
    return done;
  }
}

Future<void> _workerMain((SendPort, String, String) args) async {
  final (main, variantId, url) = args;
  final variant = Variant.parse(variantId);
  final commands = ReceivePort();
  main.send(commands.sendPort);

  // Batch outgoing messages: one port message per socket read / microtask.
  var batch = <Object?>[];
  var flushScheduled = false;
  void flush() {
    flushScheduled = false;
    if (batch.isEmpty) return;
    main.send(batch);
    batch = <Object?>[];
  }

  void deliver(Object item, {bool autoFlush = false}) {
    batch.add(item);
    if (autoFlush && !flushScheduled) {
      flushScheduled = true;
      scheduleMicrotask(flush);
    }
  }

  final uri = Uri.parse(url);
  late final void Function(Object) send;
  late final Future<void> Function() close;
  late final Future<void> done;
  late final int? Function() closeCode;

  try {
    if (variant == Variant.b) {
      final ws = await WebSocket.connect(
        url,
        compression: CompressionOptions.compressionOff,
        customClient: _insecureHttpClient(),
      );
      final completer = Completer<void>();
      ws.listen(
        (Object? data) => switch (data) {
          String text => deliver(jsonDecode(text) as Object, autoFlush: true),
          Uint8List bytes =>
            deliver(TransferableTypedData.fromList([bytes]), autoFlush: true),
          _ => null,
        },
        onDone: () {
          flush();
          completer.complete();
        },
        onError: (Object _) {},
      );
      send = ws.add;
      close = () => ws.close(1000);
      done = completer.future;
      closeCode = () => ws.closeCode;
    } else {
      final raw = variant == Variant.cWorkerRaw;
      final tape = variant == Variant.cWorkerTape;
      final model = variant == Variant.cWorkerModel;
      final ws = await WsCodecClient.connect(
        uri,
        allowBadCertificates: true,
        onMessage: (opcode, payload) {
          if (opcode == opBinary) {
            deliver(TransferableTypedData.fromList([payload]));
          } else if (raw) {
            deliver((TransferableTypedData.fromList([payload]),));
          } else if (model) {
            deliver(Order.fromJson(decodeJsonBytes(payload)! as Map<String, Object?>));
          } else if (tape) {
            deliver((tape: TransferableTypedData.fromList([buildTape(payload)])));
          } else {
            deliver(decodeJsonBytes(payload)!);
          }
        },
        onReadEnd: flush,
      );
      send = (m) => switch (m) {
            Uint8List bytes => ws.sendBinary(bytes),
            String text => ws.sendText(text),
            _ => ws.sendUtf8(encodeJsonBytes(m)),
          };
      close = ws.close;
      done = ws.done.then((_) => flush());
      closeCode = () => ws.closeCode;
    }
  } on Object catch (e) {
    main.send(('error', '$e'));
    commands.close();
    return;
  }

  main.send(('open',));
  commands.listen((Object? command) {
    switch (command) {
      case 'close':
        close();
      case TransferableTypedData data:
        send(data.materialize().asUint8List());
      case Object message:
        // Variant B sends JSON as text; the codec encodes straight to bytes.
        send(variant == Variant.b ? jsonEncode(message) : message);
    }
  });
  await done;
  commands.close();
  Isolate.exit(main, ('closed', closeCode()));
}
