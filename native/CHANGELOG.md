## 2.0.0-dev.1

- First release: the Rust transport for `ws` 2.0, with prebuilt libraries
  bundled through a build hook.
- TLS: the system trust store (OS verifier on Apple and Windows), pins,
  user CAs on Android on request, disabled system CAs honoured.
- An invalid close code from the server (999, 1005) is reported as a
  protocol violation (1002 sent by the client), not as the server
  closing with 1002.
- `LICENSE` includes the notices of the Rust crates in the libraries
  (shown by Flutter's `showLicensePage`).
