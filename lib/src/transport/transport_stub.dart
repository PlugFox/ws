import 'package:ws/src/transport/transport.dart';

/// No transport on this platform.
WebSocketTransport createPlatformTransport() =>
    throw UnsupportedError('No WebSocket transport for this platform');
