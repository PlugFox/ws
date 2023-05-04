export 'package:ws/src/platform/platform.i.dart';
export 'package:ws/src/platform/platform.stub.dart'
    // ignore: uri_does_not_exist
    if (dart.library.html) 'package:ws/src/platform/platform.html.dart'
    // ignore: uri_does_not_exist
    if (dart.library.io) 'package:ws/src/platform/platform.io.dart';
