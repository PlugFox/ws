import 'dart:convert' show base64;
import 'dart:typed_data';

/// Decodes `WebSocketTls.pins`: base64 SHA-256 digests (32 bytes), with
/// an optional `sha256/` prefix. Throws a [FormatException] naming the
/// first invalid pin.
List<Uint8List> pinDigests(List<String> pins) => [
  for (final pin in pins) _digest(pin),
];

Uint8List _digest(String pin) {
  final text = pin.startsWith('sha256/') ? pin.substring(7) : pin;
  try {
    final digest = base64.decode(text);
    if (digest.length == 32) return digest;
  } on FormatException {
    // Reported below with the pin.
  }
  throw FormatException(
    'Expected the base64 SHA-256 (32 bytes) of a SubjectPublicKeyInfo',
    pin,
  );
}
