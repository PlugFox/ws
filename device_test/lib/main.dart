import 'package:flutter/widgets.dart';

/// The app is only a host for `integration_test/`; see README.md.
void main() => runApp(
  const Directionality(
    textDirection: TextDirection.ltr,
    child: Center(child: Text('ws device tests')),
  ),
);
