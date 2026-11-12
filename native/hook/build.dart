// Provides the native library of ws_native as a code asset.
//
// Where the library comes from (DECISIONS.md D9), in order:
//
// 1. The app's override, in its pubspec.yaml:
//
//      hooks:
//        user_defines:
//          ws_native:
//            source: none | build | prebuilt   # default: auto
//            library: path/to/libws_native.so  # use this file instead
//
//    `none` ships no library: ws falls back to its pure-Dart transport.
// 2. The prebuilt library bundled in the package (`prebuilt/<target>/`),
//    verified against `prebuilt/checksums.json` before it is used.
// 3. `cargo build` of `rust/`, only when no prebuilt library exists for the
//    target (a git or path dependency, a development checkout) and cargo
//    is installed.
// 4. Nothing: no asset, and ws uses the pure-Dart transport.
//
// The hook only fails on a checksum mismatch (corruption or tampering).
// Environment variables are not visible to hooks, so overrides go through
// user-defines.
import 'dart:convert';
import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:crypto/crypto.dart';
import 'package:hooks/hooks.dart';

const _assetName = 'ws_native';

void main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;
    final code = input.config.code;
    final source = input.userDefines['source'] as String? ?? 'auto';
    if (source == 'none') return;

    final fileName = code.targetOS.dylibFileName(_assetName);
    final target = _targetName(code);
    final crate = input.packageRoot.resolve('rust/');
    // Register inputs on every path, also when nothing is built, so edits
    // re-run the hook. Only existing files: hooks_runner treats a missing
    // dependency as "modified just now", which forces a rebuild (and a cargo
    // run) on every build (DECISIONS.md D29).
    output.dependencies.addAll([
      for (final uri in [
        input.packageRoot.resolve('prebuilt/checksums.json'),
        input.packageRoot.resolve('prebuilt/$target/$fileName'),
        crate.resolve('Cargo.toml'),
        crate.resolve('Cargo.lock'),
        crate.resolve('build.rs'),
      ])
        if (File.fromUri(uri).existsSync()) uri,
      if (Directory.fromUri(crate.resolve('src/')).existsSync())
        for (final f in Directory.fromUri(crate.resolve('src/')).listSync())
          if (f is File) f.uri,
    ]);

    Uri? library = input.userDefines.path('library');
    if (library != null) output.dependencies.add(library);
    if (library == null && source != 'build') {
      library = await _prebuilt(input, target, fileName);
      if (library == null && source == 'prebuilt') {
        throw StateError('ws_native: no prebuilt library for $target');
      }
    }
    if (library == null && source != 'prebuilt') {
      library = await _cargoBuild(
        input,
        output,
        code,
        fileName,
        required: source == 'build',
      );
    }
    if (library == null) return; // pure-Dart fallback at run time

    output.assets.code.add(
      CodeAsset(
        package: input.packageName,
        name: _assetName,
        linkMode: DynamicLoadingBundled(),
        file: library,
      ),
    );
  });
}

/// `<os>-<arch>[-sim]`, the directory name under `prebuilt/`.
String _targetName(CodeConfig code) {
  final sim =
      code.targetOS == OS.iOS && code.iOS.targetSdk == IOSSdk.iPhoneSimulator;
  return '${code.targetOS.name}-${code.targetArchitecture.name}'
      '${sim ? '-sim' : ''}';
}

Future<Uri?> _prebuilt(HookInput input, String target, String name) async {
  final dir = input.packageRoot.resolve('prebuilt/');
  final file = File.fromUri(dir.resolve('$target/$name'));
  if (!file.existsSync()) return null;
  final sums = File.fromUri(dir.resolve('checksums.json'));
  if (!sums.existsSync()) {
    throw StateError('ws_native: prebuilt/checksums.json is missing');
  }
  final expected =
      (jsonDecode(await sums.readAsString())
          as Map<String, Object?>)['$target/$name'];
  final actual = sha256.convert(await file.readAsBytes()).toString();
  if (expected != actual) {
    throw StateError(
      'ws_native: checksum mismatch for prebuilt/$target/$name '
      '(expected $expected, got $actual). The package archive is corrupt '
      'or was modified.',
    );
  }
  return file.uri;
}

Future<Uri?> _cargoBuild(
  HookInput input,
  BuildOutputBuilder output,
  CodeConfig code,
  String fileName, {
  required bool required,
}) async {
  final crate = input.packageRoot.resolve('rust/');
  if (!File.fromUri(crate.resolve('Cargo.toml')).existsSync()) {
    if (required) throw StateError('ws_native: rust/ is missing');
    return null;
  }
  final String triple;
  final Map<String, String> environment;
  try {
    triple = _rustTriple(code);
    environment = _environment(code, triple);
  } on Object {
    // Unsupported target or missing toolchain: pure-Dart fallback in auto
    // mode (DECISIONS.md D9 item 4).
    if (required) rethrow;
    return null;
  }
  // Shared directory as the cargo cache; the result is copied per config,
  // because cargo does not fingerprint deployment targets or NDK levels.
  final targetDir = input.outputDirectoryShared.resolve('cargo/');
  final ProcessResult result;
  try {
    result = await Process.run(
      'cargo',
      [
        'build',
        '--release',
        '--locked',
        '--target',
        triple,
        '--target-dir',
        targetDir.toFilePath(),
      ],
      workingDirectory: crate.toFilePath(),
      environment: environment,
    );
  } on ProcessException {
    // No cargo on this machine.
    if (required) rethrow;
    return null;
  }
  if (result.exitCode != 0) {
    if (!required) {
      stderr.writeln(
        'ws_native: cargo build failed, using the pure-Dart '
        'transport.\n${result.stderr}',
      );
      return null;
    }
    throw ProcessException(
      'cargo',
      ['build'],
      '${result.stdout}\n${result.stderr}',
      result.exitCode,
    );
  }
  final built = File.fromUri(targetDir.resolve('$triple/release/$fileName'));
  final copy = File.fromUri(input.outputDirectory.resolve(fileName));
  await built.copy(copy.path);
  return copy.uri;
}

/// Linker and C compiler for Android (ring has C code), the minimum iOS
/// version, and Apple linker flags (DECISIONS.md D9).
Map<String, String> _environment(CodeConfig code, String triple) {
  final env = <String, String>{};
  final key = triple.toUpperCase().replaceAll('-', '_');
  final ccKey = triple.replaceAll('-', '_');
  switch (code.targetOS) {
    case OS.android:
      final compiler = code.cCompiler?.compiler;
      if (compiler == null) {
        throw StateError('ws_native: Android build needs the NDK compiler');
      }
      final api = code.android.targetNdkApi;
      final prefix = triple == 'armv7-linux-androideabi'
          ? 'armv7a-linux-androideabi'
          : triple;
      final clang = compiler.resolve('$prefix$api-clang').toFilePath();
      env['CARGO_TARGET_${key}_LINKER'] = clang;
      env['CC_$ccKey'] = clang;
      env['AR_$ccKey'] = compiler.resolve('llvm-ar').toFilePath();
    case OS.iOS || OS.macOS:
      if (code.targetOS == OS.iOS) {
        final v = code.iOS.deploymentVersion;
        env['IPHONEOS_DEPLOYMENT_TARGET'] = '${v.major}.${v.minor}';
      } else {
        final v = code.macOS.deploymentVersion;
        env['MACOSX_DEPLOYMENT_TARGET'] = '${v.major}.${v.minor}';
      }
    // Linker flags (16 KB pages, headerpad, install name): rust/build.rs.
    default:
  }
  return env;
}

String _rustTriple(CodeConfig code) {
  final sim =
      code.targetOS == OS.iOS && code.iOS.targetSdk == IOSSdk.iPhoneSimulator;
  return switch ((code.targetOS, code.targetArchitecture)) {
    (OS.macOS, Architecture.arm64) => 'aarch64-apple-darwin',
    (OS.macOS, Architecture.x64) => 'x86_64-apple-darwin',
    (OS.linux, Architecture.x64) => 'x86_64-unknown-linux-gnu',
    (OS.linux, Architecture.arm64) => 'aarch64-unknown-linux-gnu',
    (OS.windows, Architecture.x64) => 'x86_64-pc-windows-msvc',
    (OS.windows, Architecture.arm64) => 'aarch64-pc-windows-msvc',
    (OS.android, Architecture.arm64) => 'aarch64-linux-android',
    (OS.android, Architecture.arm) => 'armv7-linux-androideabi',
    (OS.android, Architecture.x64) => 'x86_64-linux-android',
    (OS.iOS, Architecture.arm64) =>
      sim ? 'aarch64-apple-ios-sim' : 'aarch64-apple-ios',
    (OS.iOS, Architecture.x64) => 'x86_64-apple-ios',
    (final os, final arch) => throw UnsupportedError(
      'ws_native: unsupported target $os $arch',
    ),
  };
}
