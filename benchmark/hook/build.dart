// Builds the Rust crate in `native/` with cargo and bundles it as a code
// asset for `package:ws_benchmark/ws_bench_native`.
import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';

void main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;
    final code = input.config.code;
    final triple = _rustTriple(code.targetOS, code.targetArchitecture,
        iOSSimulator: code.targetOS == OS.iOS &&
            code.iOS.targetSdk == IOSSdk.iPhoneSimulator);
    final crate = input.packageRoot.resolve('native/');
    final targetDir = crate.resolve('target/');

    final result = await Process.run(
      'cargo',
      ['build', '--release', '--target', triple, '--target-dir', targetDir.toFilePath()],
      workingDirectory: crate.toFilePath(),
      environment: _environment(code, triple),
    );
    if (result.exitCode != 0) {
      throw ProcessException('cargo', ['build'], '${result.stdout}\n${result.stderr}', result.exitCode);
    }

    final library = targetDir.resolve(
      '$triple/release/${code.targetOS.dylibFileName('ws_bench_native')}',
    );
    output.assets.code.add(
      CodeAsset(
        package: input.packageName,
        name: 'ws_bench_native',
        linkMode: DynamicLoadingBundled(),
        file: library,
      ),
    );
    output.dependencies.addAll([
      crate.resolve('Cargo.toml'),
      crate.resolve('Cargo.lock'),
      for (final dir in [crate.resolve('src/'), crate.resolve('../../server/src/')])
        for (final f in Directory.fromUri(dir).listSync())
          if (f is File) f.uri,
      crate.resolve('../../server/Cargo.toml'),
    ]);
  });
}

/// Points cargo (linker) and `cc` (C code of `ring`) at the NDK clang for
/// Android, and passes the minimum iOS version.
Map<String, String> _environment(CodeConfig code, String triple) {
  final env = <String, String>{};
  if (code.targetOS == OS.android) {
    final compiler = code.cCompiler?.compiler;
    if (compiler == null) throw StateError('Android build needs the NDK C compiler');
    final api = code.android.targetNdkApi;
    final prefix = triple == 'armv7-linux-androideabi' ? 'armv7a-linux-androideabi' : triple;
    final clang = compiler.resolve('$prefix$api-clang').toFilePath();
    final key = triple.toUpperCase().replaceAll('-', '_');
    env['CARGO_TARGET_${key}_LINKER'] = clang;
    env['CC_${triple.replaceAll('-', '_')}'] = clang;
    env['AR_${triple.replaceAll('-', '_')}'] = compiler.resolve('llvm-ar').toFilePath();
  } else if (code.targetOS == OS.iOS) {
    env['IPHONEOS_DEPLOYMENT_TARGET'] = '${code.iOS.deploymentVersion.major}.${code.iOS.deploymentVersion.minor}';
  }
  return env;
}

String _rustTriple(OS os, Architecture arch, {bool iOSSimulator = false}) =>
    switch ((os, arch)) {
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
        iOSSimulator ? 'aarch64-apple-ios-sim' : 'aarch64-apple-ios',
      (OS.iOS, Architecture.x64) => 'x86_64-apple-ios',
      _ => throw UnsupportedError('Unsupported target: $os $arch'),
    };
