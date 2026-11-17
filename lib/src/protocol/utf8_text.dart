import 'dart:convert' show utf8;
import 'dart:typed_data';

import 'package:ws/src/protocol/utf8_validator.dart';

/// Decodes a UTF-8 text message to a `String`, throwing `FormatException`
/// on invalid input.
///
/// Pure-ASCII text (most JSON, ids, protocol messages) is turned into a
/// string with `String.fromCharCodes`, which on the VM copies bytes into a
/// one-byte string: 10–12× faster than `utf8.decode` (DECISIONS.md D19).
String decodeUtf8(Uint8List bytes) => Utf8Validator.isAscii(bytes)
    ? String.fromCharCodes(bytes)
    : utf8.decode(bytes);
