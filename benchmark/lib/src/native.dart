/// Bindings to the Rust transport in `native/` (variant "d"), bundled by
/// `hook/build.dart` as the code asset `package:ws_benchmark/ws_bench_native`.
@DefaultAsset('package:ws_benchmark/ws_bench_native')
library;

import 'dart:async';
import 'dart:collection';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'clients.dart';
import 'tape.dart';

@Native<Bool Function(Pointer<Void>, Uint32)>(symbol: 'ws_native_init')
external bool _init(Pointer<Void> postCObject, int threads);

@Native<Uint64 Function(Pointer<Uint8>, Int64, Uint32)>(
  symbol: 'ws_native_connect',
  isLeaf: true,
)
external int _connect(Pointer<Uint8> url, int port, int mode);

@Native<Bool Function(Uint64, Pointer<Uint8>, Size, Bool)>(
  symbol: 'ws_native_send',
  isLeaf: true,
)
external bool _send(int id, Pointer<Uint8> data, int length, bool binary);

@Native<Bool Function(Uint64, Uint16)>(symbol: 'ws_native_close', isLeaf: true)
external bool _close(int id, int code);

/// How the Rust side delivers JSON text messages.
enum NativeMode {
  /// Raw bytes as external typed data; JSON decoded on main.
  bytes(0),

  /// simd-json → tape; lazy views on main.
  tape(1),

  /// simd-json → `Dart_CObject` tree; the VM rebuilds it on main.
  cobject(2);

  const NativeMode(this.code);
  final int code;
}

bool _initialized = false;

final class NativeClient implements BenchClient {
  NativeClient._();

  late final int _id;
  final Completer<void> _done = Completer<void>();
  int? _closeCode;

  static Future<BenchClient> connect(
    Uri uri,
    NativeMode mode,
    void Function(Object message) onMessage,
  ) async {
    if (!_initialized) {
      final threads = int.tryParse(Platform.environment['WS_NATIVE_THREADS'] ?? '') ?? 2;
      _init(NativeApi.postCObject.cast(), threads);
      _initialized = true;
    }
    final port = RawReceivePort();
    final client = NativeClient._();
    final opened = Completer<void>();
    port.handler = (Object? event) {
      final e = event! as List<Object?>;
      switch (e[0]) {
        case 3:
          for (var i = 1; i < e.length; i += 2) {
            final item = e[i + 1];
            switch (e[i]) {
              case 0:
                onMessage(decodeJsonBytes(item! as Uint8List)!);
              case 1:
                onMessage(item! as Uint8List);
              case 2:
                onMessage(TapeDocument(item! as Uint8List).root!);
              case 3:
                onMessage(cobjectValue(item)!);
            }
          }
        case 0:
          opened.complete();
        case 1:
          client._closeCode = e[1] as int?;
          port.close();
          if (!opened.isCompleted) {
            opened.completeError(const WebSocketException('Connection failed'));
          }
          client._done.complete();
        case 2:
          stderr.writeln('native error: ${e[1]}');
      }
    };
    final url = Uint8List.fromList([...uri.toString().codeUnits, 0]);
    client._id = _connect(url.address, port.sendPort.nativePort, mode.code);
    await opened.future;
    return client;
  }

  @override
  void send(Object message) {
    final (bytes, binary) = switch (message) {
      Uint8List b => (b, true),
      _ => (encodeJsonBytes(message), false),
    };
    _send(_id, bytes.address, bytes.length, binary);
  }

  @override
  Future<void> get done => _done.future;

  @override
  int? get closeCode => _closeCode;

  @override
  Future<void> close() {
    _close(_id, 1000);
    return done;
  }
}

// --- Dart_CObject tree views ----------------------------------------------

/// Wraps a value received from a `Dart_CObject` tree: containers arrive as
/// lists whose first element is a marker (0 = array, 1 = object with
/// alternating keys and values).
Object? cobjectValue(Object? value) => switch (value) {
      List<Object?> l when l[0] == 1 => CObjectMap._(l),
      List<Object?> l => CObjectList._(l),
      _ => value,
    };

final class CObjectMap extends UnmodifiableMapBase<String, Object?> {
  CObjectMap._(this._l);

  final List<Object?> _l;
  int _hint = 1;

  @override
  int get length => (_l.length - 1) >> 1;

  int _find(Object? key) {
    final l = _l;
    final hint = _hint < l.length ? _hint : 1;
    for (var i = hint; i < l.length; i += 2) {
      if (l[i] == key) {
        _hint = i + 2;
        return i + 1;
      }
    }
    for (var i = 1; i < hint; i += 2) {
      if (l[i] == key) {
        _hint = i + 2;
        return i + 1;
      }
    }
    return -1;
  }

  @override
  Object? operator [](Object? key) {
    final i = _find(key);
    return i < 0 ? null : cobjectValue(_l[i]);
  }

  @override
  bool containsKey(Object? key) => _find(key) >= 0;

  @override
  Iterable<String> get keys sync* {
    for (var i = 1; i < _l.length; i += 2) {
      yield _l[i]! as String;
    }
  }
}

final class CObjectList extends ListBase<Object?> {
  CObjectList._(this._l);

  final List<Object?> _l;

  @override
  int get length => _l.length - 1;

  @override
  Object? operator [](int index) {
    RangeError.checkValidIndex(index, this);
    return cobjectValue(_l[index + 1]);
  }

  @override
  void operator []=(int index, Object? value) =>
      throw UnsupportedError('Cannot modify an unmodifiable list');

  @override
  set length(int newLength) =>
      throw UnsupportedError('Cannot change the length of an unmodifiable list');
}
