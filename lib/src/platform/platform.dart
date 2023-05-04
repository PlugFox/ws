export 'platform.i.dart';
export 'platform.stub.dart'
    // ignore: uri_does_not_exist
    if (dart.library.html) 'platform.web.dart'
    // ignore: uri_does_not_exist
    if (dart.library.io) 'platform.io.dart';
