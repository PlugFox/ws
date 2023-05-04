import 'package:ws/src/platform/platform.i.dart';

abstract base class WebSocketPlatformTransport$Base
    implements IWebSocketPlatformTransport {
  WebSocketPlatformTransport$Base(this.url);

  @override
  final String url;

  /* @override
  final Stream<Object> stream;

  final StreamController<Object> _controller =
      StreamController<Object>.broadcast(); */
}
