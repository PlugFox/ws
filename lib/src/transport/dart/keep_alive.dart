import 'dart:io' as io;

/// Turns on TCP keepalive for [socket] (DECISIONS.md D41), with the same
/// parameters as the native transport: probes after [idle], then every
/// `idle / 3` (at least 1 s), 3 probes; on Linux and Android
/// `TCP_USER_TIMEOUT` of `idle + 3 * interval`. Best effort: returns
/// `false` if the platform refused an option.
bool setKeepAlive(io.RawSocket socket, Duration idle) {
  final options = _Options.current;
  if (options == null) return false;
  final seconds = idle.inSeconds < 1 ? 1 : idle.inSeconds;
  final interval = seconds ~/ 3 < 1 ? 1 : seconds ~/ 3;
  try {
    socket
      ..setRawOption(
        io.RawSocketOption.fromBool(
          io.RawSocketOption.levelSocket,
          options.keepAlive,
          true,
        ),
      )
      ..setRawOption(_tcp(options.idle, seconds))
      ..setRawOption(_tcp(options.interval, interval))
      ..setRawOption(_tcp(options.count, 3));
    if (options.userTimeout case final userTimeout?) {
      socket.setRawOption(_tcp(userTimeout, (seconds + 3 * interval) * 1000));
    }
    return true;
  } on Object {
    return false;
  }
}

io.RawSocketOption _tcp(int option, int value) =>
    io.RawSocketOption.fromInt(io.RawSocketOption.levelTcp, option, value);

/// Option numbers from the platform headers.
final class _Options {
  const _Options(
    this.keepAlive,
    this.idle,
    this.interval,
    this.count, [
    this.userTimeout,
  ]);

  /// `SO_KEEPALIVE`, `TCP_KEEPIDLE` (`TCP_KEEPALIVE` on Apple),
  /// `TCP_KEEPINTVL`, `TCP_KEEPCNT`, `TCP_USER_TIMEOUT`.
  final int keepAlive, idle, interval, count;
  final int? userTimeout;

  static _Options? get current {
    if (io.Platform.isLinux || io.Platform.isAndroid) {
      return const _Options(9, 4, 5, 6, 18);
    }
    if (io.Platform.isMacOS || io.Platform.isIOS) {
      return const _Options(0x8, 0x10, 0x101, 0x102);
    }
    // Windows 10 1709+: TCP_KEEPIDLE, TCP_KEEPINTVL, TCP_KEEPCNT.
    if (io.Platform.isWindows) return const _Options(0x8, 3, 17, 16);
    return null;
  }
}
