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
      for (final f in Directory.fromUri(crate.resolve('src/')).listSync())
        if (f is File) f.uri,
    ]);
  });
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
