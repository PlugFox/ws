import 'dart:async';
import 'dart:ffi' show Abi;
import 'dart:io';

import 'package:ws/src/native/native_release.dart';
import 'package:ws/src/native/sha256.dart';

/// Where release libraries are downloaded from; `{tag}` and `{asset}` are
/// replaced. A mirror can use `http(s)://` or `file://`.
const String defaultNativeUrlPattern =
    'https://github.com/PlugFox/ws/releases/download/{tag}/{asset}';

/// The target of the running process, as named in `native/prebuilt/`
/// (`linux-x64`, `macos-arm64`, ...).
String hostNativeTarget() => Abi.current().toString().replaceAll('_', '-');

/// File name of the library on [target]'s operating system.
String nativeLibraryFileName(String target) {
  final os = target.split('-').first;
  return switch (os) {
    'windows' => 'ws_native.dll',
    'macos' || 'ios' => 'libws_native.dylib',
    _ => 'libws_native.so',
  };
}

/// Thrown when the library cannot be installed.
final class NativeInstallException implements Exception {
  /// Creates the exception.
  const NativeInstallException(this.message);

  /// What went wrong.
  final String message;

  @override
  String toString() => message;
}

/// Downloads the library for [target] into [directory], verifies its
/// SHA-256 and returns the file. A file that is already there with the
/// right hash is kept without downloading.
///
/// The expected hash is [sha256] if given, otherwise the one released with
/// this version ([hashes]). Without either, nothing is downloaded: an
/// unverified library is never installed.
Future<File> installNativeLibrary({
  required String target,
  required Directory directory,
  String urlPattern = defaultNativeUrlPattern,
  String tag = nativeReleaseTag,
  Map<String, String> hashes = nativeAssetHashes,
  String? sha256,
}) async {
  final fileName = nativeLibraryFileName(target);
  final key = '$target/$fileName';
  final expected = (sha256 ?? hashes[key])?.toLowerCase();
  if (expected == null) {
    final released = hashes.isEmpty
        ? 'none'
        : hashes.keys.map((k) => k.split('/').first).join(', ');
    throw NativeInstallException(
      'ws $wsVersion has no released native library for $target '
      '(released: $released). Pass --sha256 for a library from another '
      'source, or build it with cargo (native/rust).',
    );
  }
  final file = File('${directory.path}${Platform.pathSeparator}$fileName');
  if (file.existsSync() && await _hashOf(file.openRead()) == expected) {
    return file;
  }
  final asset = '$target-$fileName';
  final url = Uri.parse(
    urlPattern
        .replaceAll('{tag}', Uri.encodeComponent(tag))
        .replaceAll('{asset}', Uri.encodeComponent(asset)),
  );
  directory.createSync(recursive: true);
  final part = File('${file.path}.part');
  try {
    final sink = part.openWrite();
    final digest = Sha256();
    try {
      await for (final chunk in await _open(url)) {
        digest.add(chunk);
        sink.add(chunk);
      }
    } finally {
      await sink.close();
    }
    final actual = digest.close();
    if (actual != expected) {
      throw NativeInstallException(
        'Checksum mismatch for $url: expected $expected, got $actual. '
        'The download is corrupt or was modified; nothing was installed.',
      );
    }
    return part.renameSync(file.path);
  } finally {
    if (part.existsSync()) part.deleteSync();
  }
}

Future<Stream<List<int>>> _open(Uri url) async {
  if (url.scheme == 'file') return File.fromUri(url).openRead();
  final client = HttpClient()
    ..findProxy = HttpClient.findProxyFromEnvironment
    ..connectionTimeout = const Duration(seconds: 30);
  try {
    final response = await (await client.getUrl(url)).close();
    if (response.statusCode != HttpStatus.ok) {
      await response.drain<void>();
      throw NativeInstallException(
        'Download of $url failed: HTTP ${response.statusCode}',
      );
    }
    return response.timeout(const Duration(seconds: 60));
  } on SocketException catch (e) {
    throw NativeInstallException('Download of $url failed: ${e.message}');
  } on HandshakeException catch (e) {
    throw NativeInstallException('Download of $url failed: TLS: $e');
  } finally {
    client.close();
  }
}

Future<String> _hashOf(Stream<List<int>> bytes) async {
  final digest = Sha256();
  await bytes.forEach(digest.add);
  return digest.close();
}
