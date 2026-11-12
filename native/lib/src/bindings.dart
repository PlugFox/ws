// Internal library; documented at the top.
// ignore_for_file: public_member_api_docs
/// The symbols of the Rust library (`rust/src/lib.rs`) as bundled by
/// `hook/build.dart` under the asset id `package:ws_native/ws_native`.
///
/// Only their addresses are used (`Native.addressOf`): the transport lives
/// in `package:ws` and calls through function pointers, so the same code
/// serves a library loaded from a path (DECISIONS.md D30).
@DefaultAsset('package:ws_native/ws_native')
library;

import 'dart:ffi';

import 'package:ws/native.dart';

@Native<Uint32 Function()>(symbol: 'ws_native_abi_version', isLeaf: true)
external int _abiVersion();

@Native<Bool Function(Pointer<Void>, Uint32)>(symbol: 'ws_native_init')
external bool _init(Pointer<Void> postCObject, int threads);

@Native<Pointer<Uint8> Function(Size)>(symbol: 'ws_native_alloc')
external Pointer<Uint8> _alloc(int length);

@Native<Uint64 Function(Pointer<Uint8>, Size, Int64)>(
  symbol: 'ws_native_connect',
)
external int _connect(Pointer<Uint8> config, int length, int port);

@Native<Uint32 Function(Uint64, Pointer<Uint8>, Size, Bool)>(
  symbol: 'ws_native_send',
)
external int _send(int id, Pointer<Uint8> data, int length, bool binary);

@Native<Bool Function(Uint64)>(symbol: 'ws_native_ping')
external bool _ping(int id);

@Native<Bool Function(Uint64, Int32, Pointer<Uint8>, Size)>(
  symbol: 'ws_native_close',
)
external bool _close(int id, int code, Pointer<Uint8> reason, int length);

@Native<Bool Function(Uint64)>(symbol: 'ws_native_abort')
external bool _abort(int id);

@Native<Bool Function(Uint64, Int64)>(symbol: 'ws_native_grant')
external bool _grant(int id, int bytes);

@Native<Int64 Function(Uint64)>(symbol: 'ws_native_buffered')
external int _buffered(int id);

@Native<Bool Function(Uint64)>(symbol: 'ws_native_request_drain')
external bool _requestDrain(int id);

@Native<Void Function(Pointer<Void>)>(symbol: 'ws_native_finalize')
external void _finalize(Pointer<Void> token);

/// The ABI version of the bundled library. Throws [ArgumentError] if the
/// asset is missing (hook `source: none`, unsupported target).
int bundledAbiVersion() => _abiVersion();

/// Addresses of the bundled library's functions.
NativeSymbols bundledSymbols() => NativeSymbols(
  abiVersion: Native.addressOf(_abiVersion),
  init: Native.addressOf(_init),
  alloc: Native.addressOf(_alloc),
  connect: Native.addressOf(_connect),
  send: Native.addressOf(_send),
  ping: Native.addressOf(_ping),
  close: Native.addressOf(_close),
  abort: Native.addressOf(_abort),
  grant: Native.addressOf(_grant),
  buffered: Native.addressOf(_buffered),
  requestDrain: Native.addressOf(_requestDrain),
  finalize: Native.addressOf(_finalize),
);
