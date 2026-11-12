import 'dart:ffi';

import 'package:meta/meta.dart';
import 'package:ws/src/native/native_transport.dart';
import 'package:ws/src/transport/transport_registry.dart';

/// Version of the C ABI this package calls; must match `ABI_VERSION` in
/// `native/rust/src/lib.rs`.
const int nativeAbiVersion = 2;

/// The Rust library (`libws_native`), loaded and checked.
///
/// Get one with [open] (a file path; for executables built with
/// `dart compile exe`, see `dart run ws:install`) or let `ws_native` provide
/// the copy its build hook bundles. Then [install] it.
///
/// The functions are looked up and wrapped once per library; calls go
/// straight through function pointers (DECISIONS.md D30).
final class WsNativeLibrary {
  WsNativeLibrary._(NativeSymbols s)
    : _abiVersion = s.abiVersion.asFunction(isLeaf: true),
      _init = s.init.asFunction(),
      _alloc = s.alloc.asFunction(isLeaf: true),
      connect = s.connect.asFunction(isLeaf: true),
      send = s.send.asFunction(isLeaf: true),
      ping = s.ping.asFunction(isLeaf: true),
      close = s.close.asFunction(isLeaf: true),
      abort = s.abort.asFunction(isLeaf: true),
      grant = s.grant.asFunction(isLeaf: true),
      buffered = s.buffered.asFunction(isLeaf: true),
      requestDrain = s.requestDrain.asFunction(isLeaf: true),
      finalizer = NativeFinalizer(s.finalize);

  /// Wraps symbols resolved by someone else (`ws_native` takes them from
  /// its bundled code asset). Throws [UnsupportedError] if the library
  /// speaks a different ABI version.
  factory WsNativeLibrary.fromSymbols(NativeSymbols symbols) =>
      WsNativeLibrary._(symbols).._checkAbi();

  /// Loads the library from [path] and checks it.
  ///
  /// Throws [ArgumentError] if the file cannot be loaded (missing, wrong
  /// architecture, or not this library) and [UnsupportedError] if it was
  /// built for another version of `ws`.
  factory WsNativeLibrary.open(String path) {
    final lib = DynamicLibrary.open(path);
    final NativeSymbols symbols;
    try {
      symbols = NativeSymbols.lookup(lib.lookup);
      // A missing symbol is reported as ArgumentError.
      // ignore: avoid_catching_errors
    } on ArgumentError catch (e) {
      throw ArgumentError.value(
        path,
        'path',
        'not a ws_native library (${e.message})',
      );
    }
    return WsNativeLibrary.fromSymbols(symbols);
  }

  void _checkAbi() {
    final version = _abiVersion();
    if (version != nativeAbiVersion) {
      throw UnsupportedError(
        'ws_native library ABI $version, this version of ws needs '
        '$nativeAbiVersion. Use the library from the same release as ws '
        '(dart run ws:install).',
      );
    }
  }

  final int Function() _abiVersion;
  final bool Function(Pointer<Void>, int) _init;
  bool _initialized = false;

  final Pointer<Uint8> Function(int) _alloc;

  /// Copies [bytes] into a buffer owned by the library, for the calls that
  /// take ownership ([connect], [send], [close]). Dart may not pass the
  /// address of a `Uint8List` through a function pointer, and the library
  /// would copy the bytes anyway (DECISIONS.md D30).
  @internal
  Pointer<Uint8> copy(List<int> bytes) {
    final length = bytes.length;
    final buffer = _alloc(length);
    if (buffer == nullptr) throw StateError('ws_native: out of memory');
    if (length > 0) buffer.asTypedList(length).setAll(0, bytes);
    return buffer;
  }

  /// Takes ownership of a buffer from [copy].
  @internal
  final int Function(Pointer<Uint8>, int, int) connect;

  /// Takes ownership of a buffer from [copy].
  @internal
  // ignore: avoid_positional_boolean_parameters
  final int Function(int, Pointer<Uint8>, int, bool) send;

  /// Sends a protocol ping.
  @internal
  final bool Function(int) ping;

  /// Takes ownership of a buffer from [copy].
  @internal
  final bool Function(int, int, Pointer<Uint8>, int) close;

  /// Drops the connection without a closing handshake.
  @internal
  final bool Function(int) abort;

  /// Returns receive credits (bytes) to the reader.
  @internal
  final bool Function(int, int) grant;

  /// Bytes queued for sending.
  @internal
  final int Function(int) buffered;

  /// Asks for a drain event once the send queue is empty.
  @internal
  final bool Function(int) requestDrain;

  /// Aborts a connection whose Dart object was collected or whose isolate
  /// died.
  @internal
  final NativeFinalizer finalizer;

  /// Starts the library's runtime ([threads] worker threads; only the first
  /// call per loaded library has an effect) and makes its transport the
  /// default for clients created afterwards. Returns `false` and changes
  /// nothing if the runtime could not start.
  bool install({int threads = 2, int window = 8 << 20}) {
    if (!_initialized) {
      _initialized = _init(NativeApi.postCObject.cast(), threads);
      if (!_initialized) return false;
    }
    WebSocketTransports.use(transport(window: window));
    return true;
  }

  /// A transport on this library, for `WebSocketOptions.transport`.
  /// [install] must have been called once.
  NativeTransport transport({int window = 8 << 20}) {
    if (!_initialized) {
      throw StateError('Call WsNativeLibrary.install() first');
    }
    return NativeTransport(this, window: window);
  }
}

/// Addresses of the library's C functions (`native/rust/src/lib.rs`).
@immutable
final class NativeSymbols {
  /// The addresses, however they were resolved.
  const NativeSymbols({
    required this.abiVersion,
    required this.init,
    required this.alloc,
    required this.connect,
    required this.send,
    required this.ping,
    required this.close,
    required this.abort,
    required this.grant,
    required this.buffered,
    required this.requestDrain,
    required this.finalize,
  });

  /// Resolves every symbol by name with [lookup] (for example
  /// `DynamicLibrary.lookup`).
  factory NativeSymbols.lookup(
    Pointer<T> Function<T extends NativeType>(String symbol) lookup,
  ) => NativeSymbols(
    abiVersion: lookup('ws_native_abi_version'),
    init: lookup('ws_native_init'),
    alloc: lookup('ws_native_alloc'),
    connect: lookup('ws_native_connect'),
    send: lookup('ws_native_send'),
    ping: lookup('ws_native_ping'),
    close: lookup('ws_native_close'),
    abort: lookup('ws_native_abort'),
    grant: lookup('ws_native_grant'),
    buffered: lookup('ws_native_buffered'),
    requestDrain: lookup('ws_native_request_drain'),
    finalize: lookup('ws_native_finalize'),
  );

  /// `ws_native_abi_version`
  final Pointer<NativeFunction<Uint32 Function()>> abiVersion;

  /// `ws_native_init`
  final Pointer<NativeFunction<Bool Function(Pointer<Void>, Uint32)>> init;

  /// `ws_native_alloc`
  final Pointer<NativeFunction<Pointer<Uint8> Function(Size)>> alloc;

  /// `ws_native_connect`
  final Pointer<NativeFunction<Uint64 Function(Pointer<Uint8>, Size, Int64)>>
  connect;

  /// `ws_native_send`
  final Pointer<
    NativeFunction<Uint32 Function(Uint64, Pointer<Uint8>, Size, Bool)>
  >
  send;

  /// `ws_native_ping`
  final Pointer<NativeFunction<Bool Function(Uint64)>> ping;

  /// `ws_native_close`
  final Pointer<
    NativeFunction<Bool Function(Uint64, Int32, Pointer<Uint8>, Size)>
  >
  close;

  /// `ws_native_abort`
  final Pointer<NativeFunction<Bool Function(Uint64)>> abort;

  /// `ws_native_grant`
  final Pointer<NativeFunction<Bool Function(Uint64, Int64)>> grant;

  /// `ws_native_buffered`
  final Pointer<NativeFunction<Int64 Function(Uint64)>> buffered;

  /// `ws_native_request_drain`
  final Pointer<NativeFunction<Bool Function(Uint64)>> requestDrain;

  /// `ws_native_finalize`
  final Pointer<NativeFunction<Void Function(Pointer<Void>)>> finalize;
}
