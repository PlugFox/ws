import 'dart:io';

import 'package:test/test.dart';
import 'package:ws/interface.dart';
import 'package:ws/src/client/ws_options_js.dart';
import 'package:ws/src/client/ws_options_vm.dart';

void main() => group('Options', () {
      group(
        'Common',
        () {
          test('common', () {
            final options = WebSocketOptions.common(
              connectionRetryInterval: (
                min: const Duration(seconds: 1),
                max: const Duration(seconds: 5)
              ),
              protocols: const <String>['protocol'],
              timeout: const Duration(seconds: 30),
            );
            expect(options.connectionRetryInterval?.min,
                equals(const Duration(seconds: 1)));
            expect(options.connectionRetryInterval?.max,
                equals(const Duration(seconds: 5)));
            expect(options.protocols, equals(const <String>{'protocol'}));
            expect(options.timeout, equals(const Duration(seconds: 30)));
          });

          test('selector', () {
            final options = WebSocketOptions.selector(
              vm: () => WebSocketOptions.vm(
                connectionRetryInterval: (
                  min: const Duration(seconds: 1),
                  max: const Duration(seconds: 5)
                ),
                protocols: const <String>['protocol'],
                timeout: const Duration(seconds: 30),
              ),
              js: () => WebSocketOptions.js(
                connectionRetryInterval: (
                  min: const Duration(seconds: 1),
                  max: const Duration(seconds: 5)
                ),
                protocols: const <String>['protocol'],
                timeout: const Duration(seconds: 30),
              ),
            );
            expect(options.connectionRetryInterval?.min,
                equals(const Duration(seconds: 1)));
            expect(options.connectionRetryInterval?.max,
                equals(const Duration(seconds: 5)));
            expect(options.protocols, equals(const <String>{'protocol'}));
            expect(options.timeout, equals(const Duration(seconds: 30)));
          });
        },
      );

      group(
        'VM',
        () {
          test('vm', () {
            final options = WebSocketOptions.vm(
              connectionRetryInterval: (
                min: const Duration(seconds: 1),
                max: const Duration(seconds: 5)
              ),
              protocols: const <String>['protocol'],
              timeout: const Duration(seconds: 30),
              compression: CompressionOptions.compressionDefault,
              customClient: HttpClient(),
              headers: const <String, Object?>{'header': 'value'},
              userAgent: 'userAgent',
            );
            expect(options.connectionRetryInterval?.min,
                equals(const Duration(seconds: 1)));
            expect(options.connectionRetryInterval?.max,
                equals(const Duration(seconds: 5)));
            expect(options.protocols, equals(const <String>{'protocol'}));
            expect(options.timeout, equals(const Duration(seconds: 30)));
            expect(
                options,
                isA<$WebSocketOptions$VM>().having(
                  (options) => options.compression,
                  'compression',
                  isA<CompressionOptions>(),
                ));
            expect(
                options,
                isA<$WebSocketOptions$VM>().having(
                  (options) => options.customClient,
                  'compression',
                  isA<HttpClient>(),
                ));
            expect(
                options,
                isA<$WebSocketOptions$VM>().having(
                  (options) => options.userAgent,
                  'compression',
                  isNotEmpty,
                ));
          });
        },
        onPlatform: <String, Object?>{
          'browser': <Object?>[
            const Skip('This test is for VM only.'),
            // They'll be slow on browsers once it works on them.
            const Timeout.factor(2),
          ],
        },
      );

      group(
        'JS',
        () {
          test('js', () {
            final options = WebSocketOptions.js(
              connectionRetryInterval: (
                min: const Duration(seconds: 1),
                max: const Duration(seconds: 5)
              ),
              protocols: const <String>['protocol'],
              timeout: const Duration(seconds: 30),
              useBlobForBinary: true,
            );
            expect(options.connectionRetryInterval?.min,
                equals(const Duration(seconds: 1)));
            expect(options.connectionRetryInterval?.max,
                equals(const Duration(seconds: 5)));
            expect(options.protocols, equals(const <String>{'protocol'}));
            expect(options.timeout, equals(const Duration(seconds: 30)));
            expect(
                options,
                isA<$WebSocketOptions$JS>().having(
                  (options) => options.useBlobForBinary,
                  'compression',
                  isTrue,
                ));
          });
        },
        onPlatform: <String, Object?>{
          'dart-vm': <Object?>[
            const Skip('This test is for JS only.'),
            // They'll be slow on browsers once it works on them.
            const Timeout.factor(2),
          ],
        },
      );
    });
