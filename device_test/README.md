# ws_device_test

Integration tests of `ws`, `ws_native` and `ws_flutter` on real devices,
against the Rust test server (`../server`) on the development machine.
Not published.

```bash
flutter devices                              # find the device id
tool/device_test.sh <device-id>              # from the repository root
tool/device_test.sh <device-id> --user-ca    # Android, test CA installed
```

The script issues a fresh server certificate from a test CA
(`build/device_ca`, created on first use), starts the server, and passes
the address, the CA and the pin with `--dart-define`. Android reaches the
server through `adb reverse`; other devices through the machine's LAN
address (iOS asks for the local network permission on first launch).

- `integration_test/tls_test.dart`: pins and the trust store, both
  transports (DECISIONS.md D45). The user-CA case needs the CA installed
  on an Android device: `adb push build/device_ca/ca.crt /sdcard/Download/`,
  then Settings > Security > Install a certificate > CA certificate.

iOS signing: put `DEVELOPMENT_TEAM = <team id>` in
`ios/Flutter/Local.xcconfig` (gitignored).
