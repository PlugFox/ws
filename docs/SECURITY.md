# Security model

What `ws` checks, and which platform policies do **not** apply to it.

## Certificates

| Platform | Transport | Trust store |
|---|---|---|
| Web | browser | the browser's; `WebSocketTls` does not apply |
| iOS, macOS | Rust | Security.framework: system, MDM and user-trusted CAs, the user's trust settings |
| Windows | Rust | CryptoAPI, the same as other apps |
| Android | Rust and pure Dart | Conscrypt's system CAs (APEX on Android 14+), minus the ones the user disabled; plus user/MDM CAs with `trustUserCertificates` |
| Linux | Rust | system CA files (`SSL_CERT_FILE`, `SSL_CERT_DIR`); bundled Mozilla roots if there are none |
| any VM | pure Dart | `dart:io` (BoringSSL with the system roots) |

`trustedRootsPem` adds roots everywhere except the web. `allowInsecure`
accepts any chain and is for development only.

## Pins

`WebSocketTls.pins` restricts the server's **leaf** key on every native
transport (DECISIONS.md D45). Pin your own keys and keep a backup pin:
a renewal with a new key locks out every installed app until it updates.
Pins do not apply on the web.

## Policies this library does not see

`ws` opens its own sockets. Platform policies made for the platform's own
HTTP stacks therefore do not apply:

- **Android `network_security_config`**: its pin sets, per-domain trust
  anchors, `debug-overrides` and `cleartextTrafficPermitted` are not read
  (reading them needs JNI, which a library loaded through FFI does not
  have). Use `pins` and `trustUserCertificates` instead. `ws://` is not
  blocked.
- **Apple App Transport Security**: ATS covers `URLSession` and does not
  affect BSD sockets; `ws://` is not blocked.
- **Revocation and certificate transparency**: Apple and Windows check
  what the OS checks; Android and Linux check neither (future_risks 3.7).

A security review that relies on any of these policies must add the
corresponding `WebSocketTls` settings.
