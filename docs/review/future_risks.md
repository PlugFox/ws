# Future risks and what to plan for

Review of the `ws` 2.0 rewrite (branch `rewrite`, state of 2026-11-12) from
the point of view of running a WebSocket client in production on mobile,
desktop and the web. It lists what the project will run into beyond what
`ROADMAP.md` already says, and where the roadmap or the code comments are
wrong or incomplete.

How to read an item:

- **I** = impact if it happens, **L** = likelihood that users hit it
  (H / M / L).
- **Plan** = the concrete thing to add to the roadmap.
- Evidence tags: `[code]` = checked in this repository, `[doc]` = stated by
  the linked source, `[field]` = production experience, not re-verified
  here, `[verify]` = plausible, must be tested before it is relied on.

Section 17 is the prioritised "Top 20 to plan now".

---

## 1. Where the roadmap, decisions or comments are wrong or missing

| # | Where | Problem | Evidence |
|---|---|---|---|
| R1 | ROADMAP §4.6 "App Store review: plain BSD sockets are fine" | True for review, incomplete for behaviour. Apple calls BSD Sockets "acceptable" for cross-platform code but lists what you then owe: connect-by-name (done, D34), system resolver (done), Apple trust evaluation (done, D35), **socket viability monitoring**, `ne_socket_set_domains`, `NEURLFilter`, and states that such code "will still be incompatible with VPN On Demand". None of the last four is planned (§2). | `[doc]` [TN3151](https://developer.apple.com/documentation/technotes/tn3151-choosing-the-right-networking-api) |
| R2 | `native/rust/src/tls.rs` header: "so enterprise, MDM and user-installed CAs work" | False on Android: only the system directory is read. D35 says so itself. On Android, MDM-installed CAs also land in the *user* store, so managed devices behind a TLS-inspecting proxy fail (§3, §8). | `[code]` `tls.rs` `load_system_roots` |
| R3 | D15 "Native: ping/timeout runs in Rust (tokio timer), the isolate is not woken" | Not what the code does. The heartbeat is a Dart `Timer` in `client.dart` for every transport; the Rust `Config` has no heartbeat field. Consequences in §2 (iOS suspension), §6 (blocked isolate) and battery. | `[code]` `client.dart:866-900`, `lib.rs` `Config` |
| R4 | ROADMAP §4.6 "detect dead connections quickly with heartbeats and TCP keepalive" | `WebSocketOptions.heartbeat` defaults to `null`, and no transport sets `SO_KEEPALIVE` / `TCP_USER_TIMEOUT`. A default client never notices a half-open connection (§10). | `[code]` `options.dart`, grep `keepalive` |
| R5 | D9 "Linux: pin a glibc floor (cargo-zigbuild)" | Not implemented. `prebuilt.yml` builds `linux-*` on `ubuntu-22.04` with the plain gnu triple, so the floor is glibc 2.35. Debian 11, Ubuntu 20.04, RHEL 8/9 (2.28/2.34) and Amazon Linux 2 cannot load it (§4). | `[code]` `prebuilt.yml:41-42`, `tool/build_native.sh:30-31` |
| R6 | `mise.toml` pins Rust 1.99.0; CI uses `dtolnay/rust-toolchain@stable` | Released binaries are built by a floating compiler, not the pinned one; no `rust-toolchain.toml`. Builds are not reproducible and a new stable can change the binary between two releases of the same source (§15). | `[code]` `native.yml`, `prebuilt.yml:47` |
| R7 | README "Flutter" snippet: unconditional `import 'package:ws_native/ws_native.dart'` in `main()` | `ws_native.dart` imports `package:ws/native.dart`, which imports `dart:ffi`. D16 notes that `dart compile wasm` rejects any reachable `dart:ffi`. A Flutter app that also targets the web cannot follow the README (§6). | `[code]` `native/lib/ws_native.dart:16` |
| R8 | ROADMAP §4.1 "Errors as a sealed hierarchy: … TLS, DNS/connect …" | The implemented hierarchy has one `WebSocketConnectionException` for DNS, TCP and TLS; the native TLS kind becomes the string `'TLS failed: …'`. Apps cannot show "check the device clock" or "untrusted network", and D14's "TLS errors retried at max backoff" has no typed input (§8, §12). | `[code]` `exceptions.dart`, `native_transport.dart:176` |
| R9 | ROADMAP §4.8 browser limits | Lists headers, ping and `bufferedAmount` only. Missing: timer throttling in hidden tabs, bfcache, mixed content, CSP, per-browser connection limits, no way to bound message size, reconnect throttling by the browser (§5). | — |
| R10 | ROADMAP §4.4 | No outgoing fragmentation and no plan for it; servers with a frame limit below their message limit (AWS API Gateway: 32 KB frame, 128 KB message) reject large single-frame sends (§9). | `[code]` grep `fragment` in the writers; `[doc]` [AWS quotas](https://docs.aws.amazon.com/apigateway/latest/developerguide/apigateway-execution-service-websocket-limits-table.html) |
| R11 | ROADMAP §4.6 / D34 "IPv4 literal on NAT64: use host names" | Fixable on Apple platforms: `getaddrinfo` synthesizes an IPv6 address for an IPv4 literal when it is actually called. tokio short-circuits literals before `getaddrinfo` (§2). | `[doc]` [Apple IPv6 guide](https://developer.apple.com/library/archive/documentation/NetworkingInternetWeb/Conceptual/NetworkingOverview/UnderstandingandPreparingfortheIPv6Transition/UnderstandingandPreparingfortheIPv6Transition.html) |
| R12 | ROADMAP §8 Risks | Missing risk classes: platform policy drift (Android 17 local network permission, iOS privacy rules), the security-response duty created by bundling binaries, one client type per socket (no mixed text + binary), ecosystem interop (`package:web_socket`, `web_socket_channel`), observability, behaviour divergence between a mutable `Map` (Dart/web) and a read-only tape view (native). | — |
| R13 | ROADMAP §6.4 "Prebuilt download: offline builds, proxy environments" | Stale after D31 (nothing is downloaded by the hook). Applies now only to `dart run ws:install`. | D31, D32 |
| R14 | `WebSocketState.toString()` | Prints the full URL including query and userinfo. Tokens in `?token=` (the only option on the web) end up in logs and crash reports (§11). | `[code]` `state.dart:98,116` |
| R15 | Default `maxMessageSize = 64 MiB` | Large for phones, and unenforceable in the browser (the message is already in memory when the event fires). The three transports do not have the same safety property (§11). | `[code]` `options.dart` |

---

## 2. iOS

- **2.1 Suspension kills the socket; the client must treat resume as a
  first-class event.** iOS suspends the app seconds after it leaves the
  foreground; the kernel may reclaim the socket, and the server side sees
  nothing until its own timeout. On resume all Dart timers fire at once.
  I:H L:H. Plan: (a) ship the Flutter lifecycle integration with 2.0, not
  "later" (`ws_flutter` or a documented `WidgetsBindingObserver` recipe):
  `paused` → stop heartbeat and reconnect timers, optionally close with
  1001; `resumed` → probe once, reconnect immediately without backoff;
  (b) a core API for it (`client.suspend()` / `client.resume()` or
  `reconnect(immediate: true, resetBackoff: true)`), so non-Flutter hosts
  can do the same; (c) test T-resume: 30 s, 5 min, 2 h in background on a
  real device. `[field]`; background rules in
  [TN2277](https://developer.apple.com/library/archive/technotes/tn2277/_index.html).
- **2.2 Monotonic clocks do not advance while the device sleeps.**
  `Stopwatch`/`clock.stopwatch()` and Rust `Instant` (CLOCK_UPTIME_RAW on
  Apple, CLOCK_MONOTONIC on Android/Linux) stop during sleep. D15's "after
  a long timer gap send one probe" cannot be detected with the monotonic
  clock alone; `stableAfter`, RPC deadlines and `rpcTimeout` stretch by the
  sleep time. I:M L:H. Plan: detect the gap by comparing wall-clock and
  monotonic deltas (wall clock can jump, so use it only as a hint), and
  drive the real decision from the lifecycle event (2.1). Add a fake-clock
  test with a wall/monotonic skew. `[field]`
- **2.3 No background keep-alive is possible; say so.** Background
  `URLSession` does not carry WebSockets, `beginBackgroundTask` gives about
  30 s, VoIP push is restricted to calls. Users will ask for "keep the
  socket in background". I:M L:H. Plan: a docs page "Background on iOS and
  Android": use APNs/FCM to wake, reconnect and resync on resume; expose
  the missed-interval so apps can fetch a delta. `[doc]`
  [TN3151](https://developer.apple.com/documentation/technotes/tn3151-choosing-the-right-networking-api)
  (background sessions are HTTP-only).
- **2.4 BSD sockets are incompatible with VPN On Demand.** A per-app or
  on-demand VPN is not brought up by a `connect()` on a raw socket; the
  enterprise app works in Safari and with `URLSession` but `ws` times out.
  Same for both the Rust and the pure-Dart transport. I:H for enterprise
  apps, L:M. Plan: document it; offer an escape hatch on Apple platforms:
  either a third transport package over `Network.framework` /
  `URLSessionWebSocketTask` (or an adapter over
  `cupertino_http`'s `CupertinoWebSocket`), selectable through the
  existing transport registry. The API already allows it; decide whether it
  is in scope for 2.x. `[doc]` TN3151 "Connect by name".
- **2.5 Socket viability after a network change.** Apple: without
  `SCNetworkReachabilityCreateWithAddressPair` "your program won't notice
  that a connection is stuck due to a TCP/IP stack reconfiguration". Today
  only the (off-by-default) heartbeat notices a Wi-Fi → cellular switch.
  I:H L:H. Plan: heartbeat on by default on native (§10.1); in the Flutter
  layer, a path monitor (`NWPathMonitor` through a plugin or
  `connectivity_plus`) that calls `client.reconnect()` on interface change;
  optionally the viability API inside the Rust library (pure C, no JNI
  problem on Apple). `[doc]` TN3151 "BSD Sockets best practices".
- **2.6 Low Data Mode, expensive paths, Wi-Fi Assist, multipath.**
  `allowsConstrainedNetworkAccess` / `allowsExpensiveNetworkAccess` exist
  only in URLSession and Network.framework; raw sockets ignore the user's
  Low Data Mode. I:L (policy, not breakage) L:M. Plan: expose
  `isConstrained` / `isExpensive` to the app through the Flutter layer and
  document "lengthen the heartbeat, drop optional subscriptions"; do not
  claim compliance. `[doc]`
  [NWPath.isConstrained](https://developer.apple.com/documentation/network/nwpath/isconstrained).
- **2.7 IPv6-only / NAT64.** Host names work (D34). IPv4 literals do not,
  on either transport; App Review tests on NAT64 and carriers (T-Mobile US
  and others) are IPv6-only. I:H when hit, L:L–M (dev/staging URLs, IoT,
  self-hosted). Plan: on Apple targets pass literals through
  `getaddrinfo` (direct `libc` call, not `AI_NUMERICHOST`) so the OS
  synthesizes the address; add a CI/manual test on a macOS "Create NAT64
  Network" hotspot; `assert`/log when a URL contains an IPv4 literal on
  iOS. See R11.
- **2.8 Local Network privacy (iOS 14+, macOS 15+).** The first connection
  to a LAN address shows the system prompt; without
  `NSLocalNetworkUsageDescription` or after "Don't Allow" the connect fails
  with a plain socket error (typically "No route to host") `[verify]` and
  there is no API to ask the state. I:H for LAN/IoT/dev-server apps, L:M.
  Plan: map the errno to a hint in the exception message ("local network
  access may be denied"), never auto-retry it at minimum backoff, document
  the Info.plist key, and add a real-device test. `[doc]`
  [TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy).
- **2.9 Privacy manifest and required-reason APIs.** Apple: "For each
  executable or dynamic library in an app that uses a required reason API,
  the bundle that includes the executable or dynamic library needs to
  include a privacy manifest file". Flutter wraps `libws_native.dylib` in a
  framework that a build hook cannot add resources to. Rust `std` on Apple
  references time and file-metadata functions that are in the System boot
  time and File timestamp categories `[verify]`. I:H (upload rejected with
  ITMS-91053) L:M. Plan: (a) in the release workflow run
  `nm -u` on the iOS dylib and fail on symbols from Apple's list, or record
  them; (b) upload a test app to TestFlight with `ws_native` before 2.0.0
  (ROADMAP §6.4 has the upload, add this check explicitly); (c) document
  the exact `NSPrivacyAccessedAPITypes` entries the app must add; (d) file
  a Flutter issue for privacy manifests in native-asset frameworks if the
  upload warns. `[doc]`
  [Describing use of required reason API](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api).
- **2.10 Export compliance.** rustls + ring is "an industry-standard
  algorithm not supplied by the operating system" in Apple's
  questionnaire; France needs a declaration in that category. Flutter apps
  are already in it (the engine bundles BoringSSL for `dart:io`), so the
  delta is small, but people will ask. I:L L:M. Plan: one paragraph in the
  docs. `[doc]`
  [Export compliance documentation](https://developer.apple.com/help/app-store-connect/reference/export-compliance-documentation-for-encryption).
- **2.11 Network Extension content filters and URL filtering.** Apple asks
  socket-based URL loaders to call `ne_socket_set_domains` and consider
  `NEURLFilter` (added to TN3151 in 2025-09). Without the domain
  association, managed-device content filters see an anonymous flow and
  may block it. I:M (schools, enterprises) L:L. Plan: backlog item; check
  feasibility from Rust (`setsockopt`-level C API) `[verify]`.
- **2.12 App Review and "why is traffic invisible".** Reviewers and
  developers use Charles/Proxyman through the system HTTP proxy; neither
  transport honours it, and TLS interception needs the CA in the system
  store *with full trust enabled*. I:M (support load) L:H. Plan: docs
  "Debugging traffic": explicit `WebSocketProxy`, `trustedRootsPem`, and a
  `dart:developer` timeline/`postEvent` stream so DevTools shows frames
  (§10.4).
- **2.13 Simulator and architecture slices.** `ios-x64-sim` is still
  built; Apple-silicon-only toolchains are dropping Intel simulators.
  Flutter drops an architecture when dylib names differ (handled, D31).
  I:L L:L. Plan: keep building x64-sim while Flutter supports it; re-check
  per Flutter release.
- **2.14 Thread QoS and wake-ups.** Two tokio workers plus a blocking pool
  (default cap 512 threads) run at default QoS; `getaddrinfo` and
  `block_in_place` (D35) use the pool. A burst of reconnecting clients
  with a hung resolver grows threads. I:M L:L. Plan: set
  `max_blocking_threads` (for example 8) and `thread_keep_alive`; measure
  idle wake-ups per minute with Instruments (Energy Log) for an idle open
  connection; target zero periodic wake-ups besides the heartbeat.
  `[code]` `lib.rs` runtime builder has no blocking limit.

## 3. Android

- **3.1 Android 17 local network permission.** For apps targeting API 37+,
  any TCP connect to a LAN address needs the `ACCESS_LOCAL_NETWORK` runtime
  permission. "These restrictions are implemented deep in the networking
  stack, and thus they apply to all networking APIs"; a blocked TCP connect
  "will typically result in a timeout error". Play's target-API deadline
  will force API 37 on every app in 2027. I:H for LAN/IoT/dev-server apps,
  L:H. Plan: (a) call `android_getnetworkblockedreason(fd)` from Rust after
  a connect timeout to a private address and return a typed
  "local network permission" error; (b) do not burn the reconnect budget on
  it (treat as terminal until `reconnect()` is called); (c) docs with the
  manifest line and `permission_handler` call; (d) emulator test with API
  37. `[doc]`
  [Android: local network permission](https://developer.android.com/privacy-and-security/local-network-permission),
  [Flutter guide](https://docs.flutter.dev/platform-integration/android/local-network-permission).
- **3.2 Cached-apps freezer closes the socket.** On Android 14+ a cached
  process is frozen about 10 s after entering the cached state, and "if all
  processes for a particular app are frozen, the system terminates any
  active TCP sockets". The tokio threads are frozen too, so nothing runs
  until the app returns. I:H L:H. Plan: same lifecycle handling as iOS
  (2.1): on `resumed`, expect a dead socket and reconnect without backoff;
  make sure a socket error observed right after unfreeze is classified as
  "lost", not as a protocol error. `[doc]`
  [Cached apps freezer](https://source.android.com/docs/core/perf/cached-apps-freezer).
- **3.3 Doze, App Standby buckets, Battery Saver, Data Saver, OEM
  killers.** Network is cut in Doze outside maintenance windows; background
  data can be denied on metered networks; several OEMs kill background
  apps regardless. A reconnect loop in those states is pure battery drain.
  I:M L:H. Plan: cap background reconnecting (stop after N failures while
  not in the foreground, resume on lifecycle/connectivity event); document
  FCM high-priority messages as the wake-up path; test with
  `adb shell dumpsys deviceidle force-idle`, `am set-inactive`,
  `cmd netpolicy set restrict-background true`. `[doc]`
  [Doze and App Standby](https://developer.android.com/training/monitoring-device-state/doze-standby).
- **3.4 Foreground services.** Apps that must stay connected need a typed
  foreground service (Android 14+), and `dataSync` is limited to 6 hours
  per 24 h on Android 15+. The library cannot solve it, but users will
  blame it. I:M L:M. Plan: docs page with the allowed types
  (`remoteMessaging`, `connectedDevice`, `specialUse` with Play review) and
  the statement that the socket itself needs no service. `[doc]`
  [FGS types](https://developer.android.com/develop/background-work/services/fgs/service-types),
  [Android 15 dataSync timeout](https://developer.android.com/about/versions/15/behavior-changes-15#datasync-timeout).
- **3.5 `network_security_config` is ignored by both transports.** The
  Rust transport reads only the system CA directory: user and
  MDM-installed CAs (which live in the user store), `debug-overrides`,
  per-domain trust anchors and NSC pin sets do not apply, and
  user-*disabled* system CAs stay trusted. Two failure modes: (a) corporate
  TLS inspection on managed Android breaks `ws_native` while OkHttp-based
  code in the same app works; (b) a security team believes NSC pinning
  covers all traffic and it does not. I:H L:M. Plan: (1) option
  `WebSocketTls.trustUserCertificates` (default false, as Android's
  default) implemented by also reading the per-user added-CA directory and
  honouring the removed list `[verify: paths readable by apps on
  Android 14–17]`; (2) first-class SPKI pinning in `WebSocketTls` (D16
  promised it; not implemented `[code]`); (3) a "Security model" page that
  says plainly: NSC, ATS and browser policies do not apply to this
  library; (4) evaluate a JNI path later (needs a Kotlin shim, which a
  build hook cannot ship; it would have to be a Flutter plugin package).
  `[doc]`
  [Network security configuration](https://developer.android.com/privacy-and-security/security-config),
  [rustls-platform-verifier Android requirements](https://docs.rs/rustls-platform-verifier).
- **3.6 Cleartext policy.** `android:usesCleartextTraffic="false"` (the
  default since API 28) is advisory for raw sockets; `ws://` works on both
  transports. A security review may flag this as a policy bypass. I:M L:L.
  Plan: refuse `ws://` to non-loopback hosts unless
  `allowCleartext: true` (or at least log once); mirrors what Flutter did
  for `HttpClient` historically. Decide before 2.0, it is a breaking
  default later.
- **3.7 No revocation and no Certificate Transparency on Android and
  Linux.** The webpki path checks neither; Apple and Windows verifiers do
  what the OS does. The same server certificate can be accepted on Android
  and rejected on iOS. I:M L:L. Plan: document; add pins for high-value
  apps; track rustls CRL support if a customer needs it.
- **3.8 Private DNS (DoT), per-app VPN, work profile routing.** These work
  because `getaddrinfo` goes through `netd` and sockets are created in the
  app's network context. They break if anyone later adds an in-process
  resolver (hickory, DoH) "for speed". I:H if regressed, L:L. Plan: record
  as a decision: system resolver only.
- **3.9 Network callbacks.** `ConnectivityManager.NetworkCallback` needs
  JNI, which a hook-shipped library does not have. Without it a Wi-Fi →
  cellular handover is noticed only when the old network is torn down
  (socket error) or by the heartbeat; with Wi-Fi lingering that is up to
  30 s plus the heartbeat interval. I:M L:H. Plan: Flutter layer listens
  (`connectivity_plus` or own plugin) and calls `reconnect()`; in Rust,
  treat `ECONNABORTED`/`ENETDOWN`/`ENETUNREACH` as "lost, retry now".
- **3.10 16 KB pages.** The linker flag is set and checked (D9, D33). Still
  open: runtime behaviour on a real 16 KB device (allocator, `ring`
  assembly), and every future dependency that ships its own object files.
  Play has enforced it for apps targeting Android 15+ since 2025-11-01.
  I:H (upload blocked) L:L now. Plan: run the conformance suite on a 16 KB
  emulator image in CI; keep the `llvm-readelf` gate. `[doc]`
  [Support 16 KB page sizes](https://developer.android.com/guide/practices/page-sizes).
- **3.11 ABIs.** arm64-v8a, armeabi-v7a, x86_64 are built. x86 (32-bit),
  riscv64 and any future ABI fall back to pure Dart silently. I:L L:L.
  Plan: surface the fallback in `metrics.transport` (done) *and* log once
  with the reason; test that an app with `abiFilters` lacking an ABI still
  builds. Watch armv7 `ring` builds (NEON detection) on old devices.
- **3.12 R8/ProGuard, JNI.** Nothing to keep today (no Java code, and the
  library is `dlopen`ed by Dart, so `JNI_OnLoad` never runs and there is
  no `JavaVM`). The moment a Kotlin shim is added (platform verifier,
  proxy, connectivity) consumer keep-rules, `minSdk`, AGP/Kotlin version
  matrices and Gradle resolution become a permanent maintenance item.
  I:M L:M (if that path is taken). Plan: keep `ws_native` Java-free; put
  anything that needs the framework into the Flutter-only package.
- **3.13 Low-end memory.** 8 MiB receive window + 16 MiB send limit +
  64 MiB max message + tape (roughly 2–4× the text size `[verify]`) per
  connection on a 2 GB device invites the low-memory killer. I:M L:M.
  Plan: lower mobile defaults (for example 4–16 MiB max message), document
  the per-connection worst case, add a soak test with `am send-trim-memory`.
- **3.14 Play pre-launch report and Play Integrity scanners.** Play flags
  native libraries with known-vulnerable embedded versions (it has done so
  for OpenSSL, libpng, etc.). A stale `ring`/rustls in a bundled binary can
  produce a Play Console warning for every app using `ws_native`. I:M L:L.
  Plan: the advisory-to-release SLA in §15.

## 4. Desktop

- **4.1 macOS App Sandbox.** Outbound sockets need
  `com.apple.security.network.client`. Flutter's template has it in
  neither `Release.entitlements` nor (for client) `DebugProfile`; the
  connect fails with "Operation not permitted". This is the most common
  Flutter-macOS networking support question. I:M L:H. Plan: detect `EPERM`
  on connect on macOS and say "missing network.client entitlement" in the
  exception message; README section. `[doc]`
  [Flutter macOS entitlements](https://docs.flutter.dev/platform-integration/macos/building#entitlements-and-the-app-sandbox).
- **4.2 macOS code signing, hardened runtime, notarization.** Flutter
  re-signs the bundled framework. A dylib obtained with
  `dart run ws:install` and loaded by path from a notarized, hardened app
  must be signed with the same Team ID (library validation) or the app
  needs `disable-library-validation`. I:M L:L. Plan: document; consider
  shipping the macOS release asset Developer-ID signed and notarized so
  that it loads anywhere.
- **4.3 macOS 15+ Local Network privacy** applies to desktop apps and to
  CLI tools run from a terminal (the terminal app gets the prompt). Same
  handling as 2.8. `[doc]` TN3179.
- **4.4 Windows firewall.** Outbound connections do not prompt; only a
  listening socket does. The prompt appears for the *test* fake server or
  the example server if they bind a non-loopback address. I:L L:M. Plan:
  bind test servers to `127.0.0.1` only; say so in the docs.
- **4.5 Windows runtime dependencies.** A Rust `cdylib` for MSVC links the
  dynamic CRT (`VCRUNTIME140.dll`) unless built with `+crt-static`. Flutter
  apps usually ship the redistributable DLLs; a `dart compile exe` server
  on a clean Windows Server or container does not, and
  `DynamicLibrary.open` fails with an unhelpful error 126. I:M L:M. Plan:
  build Windows targets with `-C target-feature=+crt-static`; add a CI job
  that loads the DLL in `mcr.microsoft.com/windows/nanoserver`. `[verify]`
  current build flags (`build.rs` sets none for Windows `[code]`).
- **4.6 Windows: unsigned DLL.** AppLocker/WDAC and some AV products block
  or quarantine unsigned DLLs, especially ones downloaded by a tool.
  SmartScreen reputation applies to MSIX/installer content. I:M L:M in
  enterprises. Plan: Authenticode-sign Windows release assets when a
  certificate is available; until then document; make sure the fallback to
  pure Dart is clean and logged.
- **4.7 Windows on Arm.** `windows-arm64` is unverified (D33: `ring` needs
  clang). Flutter supports Windows arm64. I:L L:L. Plan: run the
  conformance suite on a `windows-11-arm` runner before listing the target.
- **4.8 Linux glibc floor.** See R5: today 2.35. I:H for servers, L:H
  (RHEL 8/9, Debian 11, Ubuntu 20.04, Amazon Linux 2 are common). Plan:
  build with `cargo-zigbuild --target x86_64-unknown-linux-gnu.2.17` (or
  2.28) as D9 intended; check with `objdump -T | grep GLIBC_` in CI and
  fail above the floor; print the required version when `dlopen` fails.
- **4.9 musl / Alpine.** No musl build. The Dart hook's target description
  has OS and architecture but, as far as I can see, no libc field
  `[verify]`, so a hook cannot pick a musl prebuilt; a glibc `.so` on
  Alpine fails at `dlopen`. I:M L:M (Alpine images are popular). Plan:
  build `*-unknown-linux-musl` with `-C target-feature=-crt-static`;
  `ws:install --target linux-x64-musl`; in the hook detect musl at run
  time (`/lib/ld-musl-*.so.1` exists) or fall back to pure Dart with a
  clear log line; add an Alpine job to CI.
- **4.10 Minimal containers (`FROM scratch`, distroless).** The official
  Dart Docker pattern copies `/runtime/` only. The Rust library also needs
  `libgcc_s.so.1` `[verify]`; no CA bundle exists (handled: falls back to
  `webpki-roots`, good); `getaddrinfo` needs `nsswitch`/resolver files (as
  `dart:io` does). I:M L:M. Plan: CI job that runs the conformance suite
  in a `scratch` image built per the Dart docs; document the extra files,
  or link `libgcc` statically.
- **4.11 Stale bundled roots.** In those containers trust comes from the
  `webpki-roots` version compiled in. An image built once and run for
  years misses root removals and additions. I:M L:L. Plan: log once when
  the fallback is used; document `SSL_CERT_FILE`.
- **4.12 Linux sandboxes and packaging.** Flatpak needs
  `--share=network`, Snap needs the `network` plug; NixOS cannot run
  downloaded binaries without patching (affects `ws:install`; the bundled
  path is fine). Symbols are namespaced `ws_native_` (good, D31); re-check
  on Linux that nothing else is exported (`nm -D --defined-only`), since
  Linux has no two-level namespace and a second Rust library in the same
  process (for example `rhttp`, `flutter_rust_bridge` plugins) could clash
  on unmangled C symbols such as `ring`'s `ring_core_*` (versioned, should
  be safe `[verify]`).
- **4.13 Other Dart targets.** linux-arm (32-bit Raspberry Pi), riscv64,
  Fuchsia: pure Dart only. Fine; list them explicitly so nobody files a
  bug.

## 5. Web

- **5.1 Timer throttling in hidden tabs.** Chrome clamps timers to 1/s in
  background tabs and, after 5 minutes hidden, to one wake-up per minute
  (WebRTC exempts a page; an open WebSocket is not a reliable exemption).
  Effects on the current code: the 16 ms `bufferedAmount` poll
  (`requestDrain`) can take up to a minute to report drain, so `push()`
  futures stall; app-level heartbeats are sent late, and a server with a
  30–60 s idle rule closes the connection; RPC timeouts fire late. I:H
  L:H. Plan: (a) on `visibilitychange` → visible, run the drain check and
  a heartbeat probe immediately; (b) make the heartbeat tolerant: decide
  "dead" from wall-clock time since last inbound data, not from a count of
  timer ticks; (c) document server-initiated pings as the robust option on
  the web (the browser answers protocol pings itself, even when timers are
  throttled); (d) optional: run timers in a dedicated Worker (not
  throttled the same way) — measure before adopting. `[doc]`
  [Chrome timer throttling](https://developer.chrome.com/blog/timer-throttling-in-chrome-88).
- **5.2 bfcache.** Chrome now closes open WebSockets when a page enters the
  back/forward cache instead of making the page ineligible. A restored
  page sees a close event (often 1006) possibly long after the fact.
  I:M L:M. Plan: listen for `pageshow` with `persisted == true` and
  `pagehide`; treat as resume (immediate reconnect, reset backoff). Add a
  browser test. `[doc]`
  [blink-dev PSA](https://groups.google.com/a/chromium.org/g/blink-dev/c/52nlr8z3Png).
- **5.3 Mobile Safari.** iOS Safari and PWAs drop sockets within seconds of
  backgrounding or screen lock, sometimes without a close event until the
  tab is foregrounded; `online`/`offline` events are unreliable
  everywhere. I:M L:H. Plan: same visibility-driven probe; test matrix
  must include real iOS Safari (BrowserStack or a device), not only Chrome
  (today: Chrome only `[code]` test config). `[field]`
- **5.4 No headers, so tokens go into the URL or subprotocol.** Both leak:
  URLs into server and proxy logs and into `WebSocketState.toString()`
  (R14); `Sec-WebSocket-Protocol` values must be echoed by the server.
  I:H L:H. Plan: document the ticket pattern (short-lived one-time ticket
  from an HTTPS call, then `?ticket=`) and first-message auth via `onOpen`
  (already supported — make it the documented default); redact query and
  userinfo in every `toString` and log line.
- **5.5 Cookies and third-party contexts.** The browser attaches cookies
  per its own rules; cross-site sockets lose them under Safari ITP and in
  embedded/third-party contexts; `SameSite` applies. Not controllable.
  I:M L:M. Plan: document; never design auth around cross-site cookies.
- **5.6 Mixed content and CSP.** `ws://` from an `https://` page is blocked
  (except loopback); `connect-src` must list the `wss://` origin; the
  constructor throws synchronously on a CSP/mixed-content block in some
  browsers and fires an async error in others. The code handles the sync
  throw `[code]`; the error text says only "Could not connect". I:M L:M.
  Plan: detect page scheme vs URL scheme up front and give a precise
  message; document CSP. dart2wasm also needs `wasm-unsafe-eval` in
  `script-src`. `[doc]`
  [MDN connect-src](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/connect-src).
- **5.7 Browser hides everything about failures.** No status, no
  distinction between DNS, TLS, 401, 403 and 503 — always 1006. On the web
  nothing is terminal (D14), so a permanently rejected client retries
  forever at `maxDelay`. I:M L:H. Plan: recommend that servers accept the
  socket and then close with a 4xxx code for auth failures (visible to the
  browser); add `maxAttempts` guidance for the web; keep
  `terminalCloseCodes` extensible for 4xxx.
- **5.8 Browser-side throttling and limits.** Firefox delays reconnects to
  a host after failures and caps total sockets (200 by default); Chrome
  caps at about 255 and queues beyond that; each tab has its own socket.
  A user with 20 tabs times N sockets hits server per-user limits. I:M
  L:M. Plan: document; out of scope for the core, but leave room for a
  `SharedWorker`/`BroadcastChannel` multiplexer package. `[field]`
  `[verify]` exact limits per browser version.
- **5.9 `bufferedAmount` only; `send()` never fails.** If the buffer cannot
  grow the browser closes the socket. `sendBufferLimit` (D36) protects, as
  long as callers go through `push`. `WebSocketStream` gives real
  backpressure but is Chromium-only. I:L L:L. Plan: keep polling; revisit
  `WebSocketStream` when it reaches Safari and Firefox. `[doc]`
  [MDN WebSocketStream](https://developer.mozilla.org/en-US/docs/Web/API/WebSocketStream).
- **5.10 No inbound size limit.** `maxMessageSize` cannot be enforced
  before the browser has allocated the message (R15). I:M L:L. Plan: check
  after receipt and close with 1009; document the difference.
- **5.11 Compression asymmetry.** Browsers always negotiate
  `permessage-deflate` when the server offers it; the native and pure-Dart
  transports never do. The same app uses several times more cellular data
  on native than on the web for JSON feeds. I:M L:M. See §9.6.
- **5.12 dart2wasm interop costs.** `JSArrayBuffer.toDart` gives a wrapper
  over the JS buffer in dart2wasm; element access goes through interop
  and has been tuned back and forth in the SDK. Parsing protobuf or
  MessagePack from that view byte by byte can be far slower than from a
  Wasm-native `Uint8List`. I:M L:M. Plan: benchmark binary decode on
  dart2wasm with the view vs one bulk copy
  (`Uint8List.fromList` / `setRange`), choose per message size, record in
  DECISIONS; re-check per Dart release. `[doc]`
  [dart2wasm typed-list refactor](https://dart.googlesource.com/sdk/+/a332c17c50341f63a034308b3fe1e6c42cd434c0),
  [WasmDataTransfer benchmark](https://dart.googlesource.com/sdk/+/base/benchmarks/WasmDataTransfer/dart/WasmDataTransfer.dart).
- **5.13 Lone surrogates.** `WebSocket.send(string)` replaces unpaired
  surrogates with U+FFFD silently; the VM transports encode or throw
  differently. I:L L:L. Plan: conformance case that pins the behaviour.
- **5.14 Hot restart on Flutter web.** A hot restart does not close JS
  `WebSocket` objects created by the previous run; connections accumulate
  during development until the browser limit, and old sockets keep
  receiving. I:M (dev only) L:H. Plan: keep a registry on `globalThis`
  (in debug builds) and close the previous generation on start; test under
  DDC.
- **5.15 Workers and service workers.** A service worker is killed when
  idle (about 30 s), so a socket there is short-lived; extension MV3
  workers need traffic every < 30 s to stay alive. Dart code can run in a
  dedicated worker, where `package:web` `WebSocket` works but
  `visibilitychange` does not exist. I:L L:L. Plan: do not touch `document`
  unguarded; document that service workers are unsupported hosts.
- **5.16 Flutter web renderers and embedding.** The transport is
  renderer-independent (CanvasKit/skwasm). Wasm builds need a WasmGC
  browser; apps keep a JS fallback, so both code paths (dart2js and
  dart2wasm) are production paths and both stay in CI (they are `[code]`).
  Multi-view / embedded Flutter shares one isolate: fine.
- **5.17 Node, Deno, Bun.** `dart.library.js_interop` is also true there;
  they have a global `WebSocket` with small differences (headers option in
  some). I:L L:L. Plan: state "browser only" or add a smoke test.

## 6. Flutter and Dart specifics

- **6.1 Web builds break on the `ws_native` import** (R7). I:H L:H for
  multi-platform apps. Plan: make `package:ws_native/ws_native.dart` a
  conditional export with a no-op stub
  (`WsNative.install() => false` on the web); add a CI job that compiles
  the README snippet with `dart compile js` and `dart compile wasm`.
- **6.2 Transport registration is per isolate.** `WebSocketTransports.use`
  sets Dart static state; every isolate (`Isolate.run`, `compute`,
  background handlers of `firebase_messaging`/`workmanager`, a second
  `FlutterEngine`) starts with the pure-Dart default until it calls
  `WsNative.install()` itself. Silent, and visible only in
  `metrics.transport`. I:M L:M. Plan: document; make `install()` cheap and
  idempotent (it is `[code]`); consider auto-install on first connect when
  `ws_native` is linked (a tiny registry hook), or a debug-mode log line
  "native library available but not installed in this isolate".
- **6.3 Process-global runtime, first caller wins.** `threads` and the
  post function are fixed by the first `init` in the process. With
  add-to-app, `FlutterEngineGroup` or several isolate groups, a later
  caller's `threads`/`window` are ignored. I:L L:M. Plan: return the
  effective configuration; document.
- **6.4 Hot restart.** Covered by D18 (post failure, finalizer, port
  probe). Still to prove: (a) a connection parked with zero credits and
  no traffic is reaped; (b) a hot restart during `onOpen`/handshake;
  (c) 200 restarts in a row leave no threads, sockets or table entries;
  (d) `NativeFinalizer`s do run at isolate shutdown on every embedder.
  I:M (dev experience; also leaks server-side sessions) L:H. Plan: an
  integration test driving `flutter run` + `r`/`R` through the VM service,
  asserting on a `ws_native_debug_stats()` export (connections, tasks,
  bytes owned by Dart).
- **6.5 Hot reload.** Code changes while a session object and closures are
  alive; listener closures keep old code. Mostly fine; the risk is static
  caches in the client (`D26` rule covers futures). Plan: one manual test
  per release.
- **6.6 The main isolate now runs on the platform thread** (merged threads
  on Android and iOS in current Flutter). Any synchronous FFI call that
  blocks, blocks the platform thread: input, platform channels, and on
  Android the ANR watchdog. Today every FFI call takes the global
  connection-table `Mutex` `[code]` (`table()`), which a tokio worker can
  hold while being descheduled (priority inversion); `ws_native_alloc` +
  copy of a multi-megabyte message is also synchronous. I:M L:L–M. Plan:
  replace the table mutex with a lock-free or sharded map (or hand the
  `Arc` to Dart as an opaque token guarded by a generation), keep critical
  sections allocation-free, and add a benchmark of worst-case FFI call
  time under load (p99.9 < 100 µs). For sends above a threshold, document
  chunking or offload. `[doc]`
  [jnigen threading notes](https://github.com/dart-lang/native/blob/main/pkgs/jnigen/doc/threading.md).
- **6.7 A blocked isolate stops the heartbeat.** Because the heartbeat is a
  Dart timer (R3), a long synchronous job on the isolate (JSON of a huge
  payload, a jank spike, a debugger breakpoint!) delays pings and can make
  the *client* declare the connection dead on the next tick, or the server
  drop it. Rust keeps answering server pings. I:M L:M. Plan: move ping
  scheduling and the liveness deadline into Rust as D15 intended (Dart only
  configures it and is told about a timeout); in the pure-Dart transport
  keep the "one probe after a gap" rule. Paused-at-breakpoint must not
  cause a reconnect storm in debug builds.
- **6.8 Tape views vs real maps: behaviour differs by transport.** Native
  JSON gives a read-only lazy view; pure Dart and web give a mutable
  `LinkedHashMap`. Code that does `json.remove('x')`, `json['y'] ??= …`,
  `putIfAbsent`, or sorts a list in place works in tests (fake transport),
  on the web and on desktop without `ws_native`, and throws
  `UnsupportedError` on the phone. I:H L:M. Plan: make the non-native
  transports return unmodifiable maps/lists too (cheap wrappers) or at
  least do so in debug mode; conformance test "mutation throws on every
  transport"; lint/doc note.
- **6.9 Tape views retained in app state.** A sub-map kept in a
  Bloc/Riverpod state pins the whole message buffer (documented in D11),
  and external memory is invisible to the Dart heap growth heuristics
  unless the external size is reported. I:M L:M. Plan: pass the real
  external size when creating external typed data so GC pressure is
  accounted `[verify]` what is passed today; DevTools memory test; make the
  deep-copy helper easy to find (`.toMap()` extension).
- **6.10 Sending tape views across isolates.** `SendPort.send(view)` or
  `compute(parse, view)` must either work (copying the buffer) or fail with
  a clear error; objects tied to native finalizers have restrictions.
  I:M L:M. Plan: test `Isolate.run(() => model(view))`,
  `SendPort.send(view)` and `Isolate.exit(port, view)`; document the
  result. `[verify]`
- **6.11 Linear key lookup.** `_find` scans keys linearly `[code]`
  (`tape.dart:154,175`). Good for small objects; an object with 50 000
  keys read with 50 lookups is 2.5 M comparisons on the UI isolate, and a
  hostile server can choose the shape (§11.3). Plan: build a lazy hash
  index above N keys (for example 32).
- **6.12 Zones and error handling.** Callbacks run in the zone of whoever
  listened or created the client; `runZonedGuarded`, Sentry and
  `FlutterError.onError` integrations depend on it. D26 fixed one case.
  Plan: a test that a throwing `onMessage` reaches the right zone handler
  for each transport; state the rule in ARCHITECTURE.
- **6.13 Add-to-app and multiple engines.** Whether build hooks run and
  bundle code assets for `flutter build aar` / `ios-framework` /
  `flutter build bundle` module flows is not something I could confirm
  `[verify]`. If they do not, `WsNative.install()` returns false and the
  app silently runs pure Dart. I:M L:M for brownfield apps. Plan: build
  the three module flavours in CI once per Flutter stable; document the
  outcome.
- **6.14 Flutter and Dart version floor.** `sdk: ^3.10.0` is the first
  release with stable hooks. Hook protocol changes (`hooks`,
  `code_assets`) between SDKs have already forced fixes (D29). I:M L:M.
  Plan: CI matrix = minimum SDK, current stable, beta; pin `hooks` /
  `code_assets` with tight ranges and widen only after the matrix is
  green.
- **6.15 `flutter test` and widget tests.** Unit tests on the host run the
  hook and load the host dylib; `fake_async` + a real native transport do
  not mix (real threads ignore fake time). Plan: document "use
  `FakeWebSocketServer` in widget tests"; make `WsNative.install()` a
  no-op under `FLUTTER_TEST` unless forced.
- **6.16 Debugging and profiling builds.** Android `debug`/`profile`
  builds load the same release-optimised `.so` (fine). Stripped binaries
  mean native crashes are unsymbolicated (§10.5).
- **6.17 `dart compile exe` users take the hook path by accident.** A
  transitive dependency on `ws_native` (for example through a future
  `ws_flutter`, or a package that depends on it for convenience) breaks
  `dart compile exe` for server code sharing that package (D8). Plan: a
  rule in the docs for package authors: depend on `ws` only; apps add
  `ws_native`.

## 7. Proxies and corporate networks

- **7.1 System proxy is not read.** (D36, known.) In corporate networks
  with no direct route, nothing connects until the app passes a proxy
  explicitly; `dart:io` `HttpClient` users are used to
  `findProxyFromEnvironment`, and the browser transport just works. I:H in
  enterprises, L:M. Plan: (a) desktop/server: honour `HTTPS_PROXY` /
  `ALL_PROXY` / `NO_PROXY` behind an opt-in
  (`WebSocketProxy.fromEnvironment()`); (b) mobile: a resolver callback
  `FutureOr<WebSocketProxy?> Function(Uri)` so the Flutter layer can ask
  the OS through a plugin; (c) macOS/Windows system settings later.
- **7.2 PAC and WPAD.** Need a JavaScript engine; only the OS can evaluate
  them (`CFNetworkExecuteProxyAutoConfigurationURL`, WinHTTP,
  Android `ProxyInfo` with a local PAC proxy on port from the framework).
  I:M L:M. Plan: explicitly out of scope for the core; the callback in
  7.1(b) lets a plugin delegate to the OS.
- **7.3 NTLM, Negotiate/Kerberos, Digest.** Only Basic is implemented. A
  407 from such a proxy is a dead end. I:M L:M in Windows shops. Plan:
  surface 407 as a typed error with the `Proxy-Authenticate` header values
  and make it terminal by default (no retry loop against a proxy that
  locks accounts after N failures); allow a caller-supplied
  `Proxy-Authorization` value for tokens obtained elsewhere.
- **7.4 Proxy credentials in memory and logs.** `WebSocketProxy.toString`
  hides them `[code]` (good). They still cross FFI as JSON. Plan: keep
  them out of any debug dump; zero the config buffer after parsing.
- **7.5 HTTPS proxies and SOCKS5.** `WebSocketProxy.parse` accepts only
  `http://` `[code]`. TLS-to-proxy and SOCKS5 (common with corporate
  Zscaler/PAC outputs and developer tools) are unsupported. I:L L:L. Plan:
  reserve the scheme field now so adding them is not a breaking change.
- **7.6 TLS-intercepting proxies.** They re-sign with a private CA: works
  where the OS store has it (Apple, Windows, Linux system store); fails on
  Android (3.5) and with pins. Some strip or break `Upgrade` on port 80,
  buffer frames, or kill idle tunnels at 30–120 s. I:H L:M. Plan:
  `trustUserCertificates`, a troubleshooting page, heartbeat default
  ≤ 25–30 s, recommend `wss://` on 443 always.
- **7.7 Captive portals.** Symptoms: TLS name mismatch, HTTP 200/302 to
  the handshake, or connect success then silence. D14 retries TLS errors at
  max backoff (good). Plan: classify "HTTP 200/30x from a non-WebSocket
  server" as retryable-slow, not as a terminal 4xx-like failure; expose a
  "probably captive portal" hint in the error.
- **7.8 Proxy + Happy Eyeballs + timeouts.** `connectTimeout` must cover
  DNS, the proxy connect, the `CONNECT` exchange, TLS and the handshake as
  one budget, and a dropped connect future must not leave a resolver
  thread or half-open tunnel behind. Plan: test with the server's proxy
  endpoint (TEST_PLAN §3) and a black-holed proxy.

## 8. TLS

- **8.1 rustls is stricter than BoringSSL.** No TLS 1.0/1.1, no RSA key
  exchange, no CBC suites, no SHA-1 signatures, certificates must have a
  SAN, and — the common one — a self-signed certificate that is its own CA
  used as the server certificate is rejected by webpki
  (`CaUsedAsEndEntity`) even when it is in `trustedRootsPem`. Embedded
  devices, routers, old appliances and many developer setups
  (`mkcert` is fine; `openssl req -x509` one-liners often are not) work
  with `dart:io` and fail with `ws_native`. I:H L:M. Plan: (a) put this in
  the conformance suite as an explicit *documented difference* with tests
  for each case; (b) clear error text with the fix; (c) per-client
  transport override is already there — document "use the pure-Dart
  transport for legacy devices"; (d) decide whether `trustedRootsPem`
  should also accept an exact end-entity match (a pinned leaf).
  `[field]`
- **8.2 Pinning is promised but absent.** D16 mentions SPKI pins;
  `WebSocketTls` has only `trustedRootsPem` and `allowInsecure` `[code]`.
  I:M L:M (banking, health). Plan: `pins: Set<String>` (SPKI SHA-256,
  base64), applied *in addition to* chain validation, with: backup pin
  required (assert ≥ 2), a distinct typed error, a callback or metric for
  pin failures, and identical behaviour in the pure-Dart transport
  (`onBadCertificate` cannot see the chain for a *valid* cert — the Dart
  side needs `SecureSocket` + `X509Certificate` post-handshake check;
  `[verify]` that the SPKI is reachable from `dart:io`).
- **8.3 Pinning UX and certificate lifetimes.** Public certificate
  lifetime dropped to 200 days in March 2026, goes to 100 days in March
  2027 and 47 days in March 2029. Leaf pins break on every renewal unless
  the key is reused; intermediates rotate too. I:H for pinning apps, L:H.
  Plan: docs: pin SPKI of keys you control (with a backup key), never the
  leaf certificate; ship a remote kill switch; treat a pin failure as
  terminal with a distinct state so the app can tell the user to update.
  `[doc]` [SC-081v3 schedule](https://thesslstore.com/blog/47-day-ssl-certificate-validity-by-2029).
- **8.4 Clock skew.** A device with a wrong clock fails every handshake
  ("not yet valid"/"expired"). It is among the top TLS failures on consumer
  Android devices. Without a typed TLS error (R8) the app cannot tell the
  user. I:M L:M. Plan: `WebSocketTlsException` with a reason enum
  (`expired`, `notYetValid`, `unknownIssuer`, `nameMismatch`, `revoked`,
  `pinMismatch`, `protocol`, `other`), mapped from rustls errors, the Apple
  and Windows verifier results and `dart:io`'s `HandshakeException` text
  (best effort). Adding it later is a breaking change to the sealed
  hierarchy (§12.1).
- **8.5 OCSP, CRL, CT.** rustls passes a stapled OCSP response to the
  verifier but fetches nothing. Apple/Windows verifiers apply OS policy
  (may block on network fetches — D35 handles the blocking); Android and
  Linux do no revocation and no CT (3.7). Must-staple is not enforced on
  the webpki path `[verify]`. I:L L:L. Plan: document the matrix per
  platform in one table.
- **8.6 Blocking verifier on a 2-thread runtime.** `block_in_place` moves
  the other tasks off the worker, but with 2 workers and several
  simultaneous handshakes waiting on a slow OCSP/AIA fetch the runtime can
  still starve; the handshake must stay under `connectTimeout`. I:M L:L.
  Plan: run verification on the blocking pool with a bound, or measure
  10 parallel handshakes with a black-holed OCSP responder.
- **8.7 Crypto provider.** `ring`: an "unmaintained" advisory was published
  and withdrawn in February 2025 when the rustls team took over
  security-only maintenance. It builds easily for mobile but has no
  post-quantum key exchange and no FIPS story; `aws-lc-rs` is the rustls
  default, larger, harder to cross-compile (CMake/NASM, Windows arm64).
  I:M L:M over 2–3 years (PQ becoming a compliance requirement). Plan:
  keep `ring` for 2.0; isolate the provider behind one function (already
  `provider()` `[code]`); re-evaluate yearly; note that enabling hybrid PQ
  enlarges the ClientHello and trips some middleboxes — make it an option
  when it comes. `[doc]`
  [RUSTSEC-2025-0007 (withdrawn)](https://rustsec.org/advisories/RUSTSEC-2025-0007.html).
- **8.8 Client certificates (mTLS).** Not in the options. Common in B2B,
  IoT and internal tools; on mobile the key often lives in the
  Keychain/Keystore and cannot be exported as PEM. I:M L:M. Plan: PEM
  key + chain option for 2.x (both transports); hardware-backed keys are a
  separate, later design (signing callback).
- **8.9 `allowInsecure` in production.** Nothing stops it. Plan:
  `assert`-level warning in debug, one log line per process in release, a
  lint in the docs; ensure it is never shared with the default config
  (tested `[code]`).
- **8.10 Session resumption and 0-RTT.** Resumption is on (D35). Tickets
  held across a network change are harmless. 0-RTT must stay off (replay).
  Plan: record as a decision.
- **8.11 ALPN and SNI.** WebSocket over HTTP/1.1 needs ALPN `http/1.1` or
  none; a server or CDN that negotiates `h2` by default when the client
  offers nothing is fine, but offering `h2` by accident breaks the
  upgrade. IP-literal hosts send no SNI; some CDNs reject that. Plan:
  conformance case asserting the offered ALPN on both transports.
- **8.12 Encrypted Client Hello, TLS fingerprinting.** Bot-protection
  products fingerprint the ClientHello (JA3/JA4). A rustls fingerprint is
  unusual for a "mobile app" and differs from the `dart:io` one, so WAF
  rules tuned to one may block the other. I:M L:L–M. Plan: mention in the
  troubleshooting page; keep a stable default cipher/extension order
  across releases and note changes in the CHANGELOG.

## 9. Servers in the wild

- **9.1 Mixed text and binary on one socket.** `WebSocketClient<T>` is
  text, JSON *or* binary. Voice/AI realtime APIs, many game and collab
  protocols send JSON control frames and binary media on the same
  connection. Today the JSON client reports binary as an error and drops
  it; the binary client decodes text as bytes and loses the type. I:H L:H
  (this is the 2025–26 growth area). Plan: a fourth flavour before the API
  freezes — `WebSocketClient.raw()` delivering a small sealed
  `WebSocketMessage` (`TextMessage(String|bytes)` / `BinaryMessage`) —
  or a codec hook `T Function(bool isText, Uint8List)`; decide now because
  codecs are private (D11).
- **9.2 Default handshake headers.** Neither `User-Agent` nor `Origin` is
  sent `[code]`. Cloudflare/Akamai bot rules and some WAFs block requests
  without a `User-Agent`; some servers require an `Origin` (allow-lists),
  others reject any `Origin` from a non-browser. I:M L:M. Plan: send
  `User-Agent: ws/<version> (dart)` by default (overridable, removable);
  never send `Origin` by default; document both.
- **9.3 Protocol stacks on top of WebSocket.** Users will try `ws` against:
  Socket.IO (Engine.IO framing, server-driven ping every 25 s, 20 s
  timeout, HTTP long-polling handshake unless `transports=websocket`),
  SignalR (JSON handshake with 0x1E record separators, server keep-alive
  15 s, client timeout 30 s), Phoenix channels (client heartbeat every
  30 s on the `phoenix` topic), Centrifugo (server ping 25 s, client must
  answer; 64 KB default message limit), STOMP, GraphQL-WS
  (`graphql-transport-ws` subprotocol), MQTT-over-WS. I:M L:H. Plan: do
  not implement them; make sure the primitives fit: (a) heartbeat mode
  "server pings, I answer, and I consider the link dead after
  `interval + grace` without a ping" (today only client-initiated ping +
  `isPong`); (b) `onOpen` for handshakes (exists); (c) subprotocols
  (exists); (d) a short "recipes" page per stack; (e) run the recipes in CI
  against Docker images of at least Centrifugo and Phoenix. `[doc]`
  [Socket.IO](https://socket.io/docs/v4/how-it-works/),
  [SignalR configuration](https://learn.microsoft.com/aspnet/core/signalr/configuration),
  [Centrifugo configuration](https://centrifugal.dev/docs/server/configuration),
  [Phoenix.Socket](https://hexdocs.pm/phoenix/Phoenix.Socket.html).
- **9.4 Frame and message limits.** AWS API Gateway: 32 KB frames, 128 KB
  messages, larger closes with 1009; Cloudflare Workers: 32 MiB per
  message since 2025-10 (1 MiB before); Centrifugo 64 KB; many servers
  default to 64 KB–1 MB. Neither writer fragments outgoing messages (R10).
  I:M L:M. Plan: `maxOutgoingFrameSize` option (fragment in the writer;
  tungstenite needs manual `Frame` writes or a wrapper `[verify]`); make
  1009 from the server a distinct, non-retried error with the message
  size in it (it is terminal by default `[code]` — good).
- **9.5 Idle timeouts on the path.** nginx `proxy_read_timeout` 60 s; AWS
  ALB 60 s; AWS NLB 350 s; AWS API Gateway 10 min idle and a hard 2 h
  connection lifetime; Azure Load Balancer 4 min; Google Cloud LB backend
  timeout 30 s by default (applies to WebSockets); Heroku 55 s; Cloudflare
  about 100 s; mobile carrier NAT anywhere from 30 s to a few minutes.
  With the heartbeat off by default (R4) an idle client is disconnected
  every minute behind half of these. I:H L:H. Plan: default heartbeat on
  for VM transports at ≤ 25 s (protocol ping costs the app nothing), with
  docs on the battery trade-off; a planned-reconnect hook for hard
  lifetimes (API Gateway's 2 h): `maxConnectionAge` with jitter so that a
  fleet does not reconnect at the same second. `[doc]`
  [nginx WebSocket proxying](https://nginx.org/en/docs/http/websocket.html),
  [ALB idle timeout](https://docs.aws.amazon.com/elasticloadbalancing/latest/application/application-load-balancers.html#connection-idle-timeout),
  [NLB idle timeout](https://docs.aws.amazon.com/elasticloadbalancing/latest/network/network-load-balancers.html#connection-idle-timeout),
  [API Gateway WebSocket quotas](https://docs.aws.amazon.com/apigateway/latest/developerguide/apigateway-execution-service-websocket-limits-table.html),
  [Azure LB idle timeout](https://learn.microsoft.com/azure/load-balancer/load-balancer-tcp-reset),
  [Google Cloud backend timeout](https://cloud.google.com/load-balancing/docs/backend-service#timeout-setting),
  [Heroku WebSocket timeouts](https://devcenter.heroku.com/articles/websockets#timeouts),
  [Cloudflare WebSockets](https://developers.cloudflare.com/network/websockets/).
- **9.6 `permessage-deflate`.** Not offered. Most servers work without it,
  but (a) browsers get it and native does not (5.11), (b) some feeds are
  sized on the assumption of compression, (c) tungstenite still has no
  merged implementation (PR #426 is a long-standing draft `[verify]`
  current state), so the Rust side would need a different codec
  (`fastwebsockets`, `yawc`, own) — which ties into ROADMAP §5.2. I:M L:M.
  Plan: reserve `compression:` in the options now (enum `none` only);
  decide the Rust codec with deflate as a criterion; when implemented:
  `client_no_context_takeover` by default on mobile, an inflate output cap
  (§11.2), Autobahn 12.*/13.*; never compress frames carrying secrets
  alongside attacker-controlled data (CRIME/BREACH) — document. `[doc]`
  [tungstenite-rs PR #426](https://github.com/snapview/tungstenite-rs/pull/426).
- **9.7 Ping policies.** Servers that ping and expect a pong in a few
  seconds: Rust answers without Dart (good), the pure-Dart transport
  answers only when the isolate is free, the browser answers itself.
  Servers that never answer client pings (some gateways swallow them):
  the heartbeat then kills healthy connections — hence "any inbound
  traffic counts" (TEST_PLAN C21, good). Servers that close on *unsolicited*
  pongs or on pings with payload: rare. Plan: T19–T22 in CI; an option to
  send empty-payload pings only (default).
- **9.8 Close-code conventions.** 1000 from the server on deploy, 1001,
  1012, 1013 with or without `Retry-After`-like reason text, application
  codes 4000–4999 with product-specific meaning (auth expired = terminal;
  "reconnect elsewhere" = immediate). Defaults are sensible (D14). Plan:
  recipes showing `shouldReconnect` for Centrifugo/Discord-style codes;
  expose the close reason string to `shouldReconnect` (it is in the
  exception `[code]`).
- **9.9 Reconnect storms.** After a server restart every client reconnects;
  `minDelay` 500 ms, `maxDelay` 15 s with full jitter is on the aggressive
  side for fleets above 10⁵ clients, and the *first* retry has only
  0–500 ms of spread. 429/503 `Retry-After` is exposed in headers but not
  honoured automatically `[verify]`. I:H for the server, L:M. Plan: honour
  `Retry-After` (bounded) on 429/503 and 1013; document tuning; consider a
  higher default `maxDelay` (30–60 s) and jitter on the first attempt
  after an *established* connection is lost.
- **9.10 Cookies and sticky sessions.** No cookie jar. ALB/ingress
  stickiness cookies and session cookies must be passed by hand through
  `request`; `Set-Cookie` on the 101 (several headers with the same name)
  cannot be represented in `Map<String, String>` without lossy folding
  `[code]` (`WebSocketHandshakeException.headers`). Plan: expose response
  headers as `Map<String, List<String>>` or special-case `set-cookie`
  before the API freezes; expose the successful handshake's response
  headers too (ROADMAP §4.4 asks for it; `onOpen(String? protocol)` has
  only the protocol `[code]`).
- **9.11 Redirects.** Not followed (D7), `location` exposed. Correct
  default. Corporate SSO gateways and "http → https" front ends answer
  301/302/307 to the handshake. Plan: classify 3xx as terminal with a
  precise message; recipe for following once in `request`; never forward
  `Authorization` across origins.
- **9.12 HTTP/2 and HTTP/3 only front ends.** RFC 8441 (WebSocket over
  h2) is not supported. A few CDNs/ingresses can be configured to refuse
  HTTP/1.1 upgrades. I:M when hit, L:L. Plan: keep out of scope; detect
  the failure signature (ALPN picks `h2`, or `HTTP_1_1_REQUIRED`/426) and
  say so.
- **9.13 Server-side masking and RSV sloppiness.** Some embedded servers
  send masked frames or set RSV bits; strict rejection (D4) is correct per
  the RFC, and browsers reject these frames too, so "it works in the
  browser" reports are unlikely. Keep strict; the error message should
  name the violation.
- **9.14 Message ordering vs. pipelining.** D28 notes that Rust keeps
  reading while Dart handles earlier messages. For request/response
  protocols that is invisible; for protocols where the client must answer
  message N before the server sends N+1 it is fine too. Record that the
  transport never reorders.

## 10. Load balancers, half-open connections, liveness

- **10.1 Heartbeat off by default** (R4). Half-open connections are the
  normal failure mode on mobile (radio loss, NAT rebinding, LB idle
  reaping without RST). Without a heartbeat or keepalive the client sits
  in `open` forever and `push` succeeds into the kernel buffer until it
  fills. I:H L:H. Plan: default `WebSocketHeartbeat()` on VM transports;
  on the web keep it opt-in (needs an app-level message) but warn in
  debug when a web client has none.
- **10.2 TCP keepalive and `TCP_USER_TIMEOUT`.** Not set `[code]`.
  Keepalive is a cheap second line (kernel-level, no wake-up of the app);
  `TCP_USER_TIMEOUT` (Linux/Android) bounds how long unacknowledged data
  may sit, which is what detects a black-holed *write* quickly. Plan: set
  `SO_KEEPALIVE` with idle 30–60 s where the platform allows per-socket
  tuning; `TCP_USER_TIMEOUT` ≈ heartbeat interval + timeout on
  Linux/Android; same options from Dart
  (`RawSocketOption`) for parity; conformance case T23 already covers the
  heartbeat path.
- **10.3 Write stall without read activity.** A sender-only client
  (telemetry upload) never notices a dead peer until the send buffer
  limit trips (`WebSocketBufferFullException`), which says "retry later",
  not "dead". Plan: when `bufferedAmount` has not decreased for
  `heartbeat.timeout` while non-zero, treat as lost.
- **10.4 LB connection draining and deploys.** Graceful servers send 1001
  or 1012 and expect clients to come back spread over the drain window;
  others just RST. Plan: for 1001/1012 reconnect with jitter over a few
  seconds (not immediately, not full backoff); document server-side
  recommendations (send 1012 with a spread).
- **10.5 DNS on reconnect.** Each attempt re-resolves (good for failover).
  `getaddrinfo` has no timeout: the dropped future leaves a blocked
  thread until the resolver gives up (30 s+ on a flapping network).
  Plan: bound blocking threads (2.14); make sure `connectTimeout` fires on
  time regardless (test with a black-holed resolver).
- **10.6 Happy Eyeballs details.** RFC 8305 also asks for a resolution
  delay (start with whichever family answers first, prefer AAAA after
  50 ms) — impossible with `getaddrinfo`, acceptable. Remember the winning
  family per host for a short time to avoid paying 250 ms on every
  reconnect on broken-IPv6 networks. I:L L:M. Plan: small per-process
  cache (host → last good family, TTL about 10 min).
- **10.7 Anycast and geo failover.** Long-lived sockets do not follow DNS
  changes; a region evacuation needs the server to close connections.
  `maxConnectionAge` (9.5) helps. Document.

## 11. Security

- **11.1 Memory limits.** `maxMessageSize` 64 MiB default; 8 MiB credit
  window; tape adds a multiple; the web cannot enforce (R15). A hostile or
  buggy server can hold tens of MB per connection. I:M L:L. Plan: smaller
  defaults on mobile, a documented worst case per connection, a test that
  a 64 MiB message never makes RSS exceed a stated bound, and check that
  fragmented messages are bounded cumulatively (they are in the Dart
  reader `[verify]` for tungstenite: `max_message_size` covers it).
- **11.2 Decompression bombs.** Not applicable until deflate exists; when
  it does, limit inflated size incrementally (abort at
  `maxMessageSize`, never inflate to completion first) and cap the
  compression ratio. Put this in the acceptance criteria of that feature
  now.
- **11.3 JSON as an attack surface.** (a) Deep nesting: simd-json and the
  Dart tape builder need a depth limit; the lazy views do not recurse on
  access, but `jsonEncode(view)`, the deep-copy helper, `toString()` and
  `==` do — a 100 000-deep array overflows the Dart stack → uncatchable
  `StackOverflowError` in the UI isolate. No depth limit is visible in
  `tape.rs` or `tape.dart` `[code]` (grep `depth`). (b) Huge key counts
  with linear lookup (6.11). (c) Duplicate keys: last-wins as `jsonDecode`
  (corpus test exists, D37). (d) Numbers: big integers, `1e400`, `-0`
  (ROADMAP §6.2). (e) Long strings of escapes. I:M L:L. Plan: depth limit
  (for example 256, configurable) on every transport's decoder including
  the web's `dart:convert` path (wrap or pre-scan), fuzz the tape reader
  with structure-aware inputs, iterative deep copy.
- **11.4 Secrets in logs.** URLs with tokens in `toString()` (R14),
  headers in exceptions, proxy credentials, the JSON config crossing FFI.
  I:H L:H. Plan: a single `redact(Uri)` used by every `toString`,
  exception message and metric; never include request headers in errors;
  test with a URL containing `token=` and userinfo.
- **11.5 FFI memory safety.** Covered by ROADMAP §6.3/§8 (Miri, sanitizers,
  fuzzing). Add: (a) Miri cannot run tokio I/O or real FFI — structure the
  crate so tape building, credit accounting and buffer ownership are pure
  functions that Miri and `cargo fuzz` can reach; (b) ASan + TSan builds
  of the conformance suite on Linux in CI (Dart VM + sanitised cdylib
  works with `LD_PRELOAD` of the runtime `[verify]`); (c) the Dart side:
  a tape view must never read past the buffer when the tape is malformed —
  fuzz `tape.dart` with corrupted tapes even though Rust "always" builds
  valid ones (the ABI is a trust boundary once libraries are loadable by
  path, D30).
- **11.6 Loading a library by path.** `WsNativeLibrary.open(path)` loads
  arbitrary code; `ws:install` verifies SHA-256 against hashes in the
  package (good). Risks: a writable install directory (TOCTOU between
  verify and load), `--url` + `--sha256` flags used to pull unofficial
  builds, a compromised release workflow (hashes and binaries come from
  the same job). I:H L:L. Plan: build provenance attestation for release
  assets (GitHub artifact attestations / SLSA) and verify it in
  `ws:install` when available; document directory permissions; two-person
  rule or environment protection on the release workflow.
- **11.7 Supply chain of the bundled binaries.** Users cannot rebuild or
  patch the Rust dependencies themselves when binaries come in the pub
  archive; see §15.2 for the response duty. Reproducible builds
  (`--remap-path-prefix`, pinned toolchain R6, `SOURCE_DATE_EPOCH`,
  `cargo build --locked`) let a third party verify the archive.
- **11.8 Cleartext and downgrade.** `ws://` allowed everywhere (3.6);
  redirects not followed (good); `request` callback could switch scheme —
  fine, it is app code. Plan: the cleartext default decision (3.6).
- **11.9 Cross-protocol and header injection.** CR/LF in headers rejected
  (D7); masking keys from a CSPRNG (D6); userinfo UTF-8 (D37). Check the
  same for subprotocol tokens and the proxy `CONNECT` authority (host
  taken from the URL — reject control characters and spaces).
- **11.10 Denial of service by control frames.** A flood of pings makes
  the client write pongs unboundedly; tungstenite coalesces/limits
  `[verify]`, the Dart writer should keep only the latest pending pong.
  Add a conformance case (ping flood with a slow reader).
- **11.11 Timing of `onOpen` auth.** Messages pushed by the server before
  `onOpen` completes: are they delivered, buffered or dropped? Decide and
  pin (an unauthenticated session should not surface data to the app as
  if it were `open`).
- **11.12 Policy.** No `SECURITY.md`, no advisory process `[code]`. Plan:
  add a security policy with a private reporting channel and a supported
  versions table before 2.0.0.

## 12. API evolution and semver

- **12.1 Sealed hierarchies freeze at 2.0.0.** `WebSocketException` and
  `WebSocketState` are sealed; adding a subtype later breaks every
  exhaustive `switch` in user code, so it needs a major version. Known
  additions already identified in this review: TLS exception (8.4), proxy
  exception (7.3), local-network/permission exception (2.8, 3.1), pin
  mismatch, a `suspended` state (2.1). I:H L:H. Plan: add them *before*
  2.0.0, or un-seal exceptions (keep `sealed` only where exhaustiveness is
  the point) and give `WebSocketConnectionException` a `kind` enum with an
  `unknown` member that can grow.
- **12.2 Enums grow too.** `WebSocketRpcFailure` and any future reason
  enum: document "new values may be added in minor releases; always have a
  default branch", or add `unknown` now.
- **12.3 The transport interface is public** (`lib/transport.dart`).
  Adding a method to `TransportConnection`/`TransportListener` breaks
  third-party transports and fakes. Plan: mark the interfaces `base`
  (extend, not implement) with default implementations, or declare the
  library "experimental, may change in minor versions" until a second
  external transport exists.
- **12.4 `ws` ↔ `ws_native` ↔ binary version skew.** Three artifacts: the
  FFI Dart code (in `ws`), the hook and symbols (in `ws_native`) and the
  binary (bundled or installed). ABI is checked at load (D30) and a
  mismatch throws `UnsupportedError` — the app does **not** fall back
  (D10: fallback only when loading fails). A `pub upgrade` that moves `ws`
  but not `ws_native` could turn a working app into one that throws at
  startup. I:H L:M. Plan: `ws_native` depends on an exact minor range of
  `ws` (`>=2.3.0 <2.4.0`) and both are released together; an ABI *range*
  (`min..=max`) negotiated at load so additive changes do not break;
  `install()` returns false and logs on mismatch rather than throwing
  (only `open(path)` throws); CI job testing N−1/N combinations.
- **12.5 Options growth.** `const` constructors with named parameters are
  additive-friendly (good). Platform parts `vm:`/`js:` exist; reserve now:
  `compression`, `tls.pins`, `tls.clientCertificate`,
  `tls.trustUserCertificates`, `proxy` as a resolver, `maxOutgoingFrameSize`,
  `maxConnectionAge`, keepalive. Changing a *default* (heartbeat on,
  cleartext refused, smaller max message) is behaviourally breaking: do it
  before 2.0.0.
- **12.6 Behavioural semver.** Reconnect timing, terminal code sets, error
  classification, ordering of state events and JSON number semantics are
  de-facto API. Plan: a "behaviour contract" section in the docs, each
  line backed by a conformance test, and CHANGELOG entries for any change.
- **12.7 pub.dev signals.** `ws` must keep the WASM-ready and all-platform
  badges (no `dart:ffi`/`dart:io` reachable from `lib/ws.dart` — guarded by
  design; add a pana run to CI). `ws_native` will show "no web support"
  unless stubbed (6.1). A 13 MB archive is downloaded by every CI run of
  every user; watch pub's limits and the false-secret validator (D31).
- **12.8 Name and discoverability.** The class `WebSocketClient` and the
  package name `ws` collide in search with `dart:io` `WebSocket` and
  `web_socket_channel` (ROADMAP §4.1 open question). Decide before 2.0.0;
  renames after are a major version.
- **12.9 Deprecation policy and support window.** State it: which 1.x
  fixes continue (security only, N months), minimum Dart SDK policy
  (latest two stables?), MSRV policy (§15.1).

## 13. Migration

- **13.1 From ws 1.x.** Different shape (typed client, no interceptors, no
  `add(Object)`, different state stream, different exception types,
  `connect` semantics, no replay). I:M L:H. Plan: a migration guide with a
  1.x → 2.0 table (M8 has it); a thin `package:ws/compat.dart` only if the
  maintainer wants a soft landing — otherwise say clearly "no shim";
  `dart fix` data cannot express this rewrite; keep 1.x on a `1.x` branch
  for security fixes with an end date.
- **13.2 Interceptors.** ROADMAP §4.9 lists them, the implementation has
  none (`onMessage` and the codec are the hooks). Either implement or
  remove from the roadmap and give recipes (logging, auth envelope,
  metrics) — 1.x users will look for them.
- **13.3 From `web_socket_channel`.** The ecosystem (GraphQL clients,
  Supabase/Phoenix/STOMP/Centrifuge clients, many in-house SDKs) takes a
  `WebSocketChannel`/`StreamChannel` or, increasingly, the
  `package:web_socket` `WebSocket` interface (implemented by
  `IOWebSocket`, `BrowserWebSocket`, `CupertinoWebSocket`,
  `OkHttpWebSocket`). Without an adapter `ws` cannot be dropped into any of
  them, which limits adoption to greenfield code. I:H for adoption, L:H.
  Plan: a small adapter package (`ws_channel` or in `ws` behind a separate
  library) exposing a `WebSocketChannel` and a `package:web_socket`
  `WebSocket` over one `ws` connection (reconnect off by default there,
  because those interfaces have no notion of it); run
  `web_socket_conformance_tests` against the adapter (TEST_PLAN already
  mines them). `[doc]`
  [package:web_socket](https://pub.dev/packages/web_socket),
  [web_socket_channel](https://pub.dev/packages/web_socket_channel).
- **13.4 Semantics people rely on elsewhere.** `web_socket_channel`
  buffers sends before open and exposes `ready`; `dart:io` `WebSocket` has
  `pingInterval`, `closeCode`, compression on by default. Migration notes
  must call out: sends before open fail fast (D12), no compression, close
  semantics, `Map` views are read-only on native (6.8).
- **13.5 Downstream packages of the maintainer (spinify).** A transitive
  `ws` 2.0 bump reaches users who never chose it; they get new defaults
  (heartbeat, TLS strictness with `ws_native`) without reading this
  documentation. Plan: downstream packages depend on `ws` only and leave
  `ws_native` to apps (6.17); release notes there link to the behaviour
  contract.

## 14. Testing strategy (beyond TEST_PLAN and ROADMAP §6)

- **14.1 Fault injection below the socket.** TEST_PLAN's server endpoints
  cover protocol faults. Network faults need a different layer: packet
  loss, latency, bandwidth, blackhole (drop without RST), RST injection,
  MTU black holes, DNS failure/slowness. Plan: `toxiproxy` in CI for
  TCP-level faults (works on GitHub runners); `tc netem` + `iptables
  -j DROP` in a privileged Linux job for blackholes and loss; on macOS
  `dnctl`/`pfctl` or Network Link Conditioner for manual runs.
- **14.2 Chaos / model-based tests.** TEST_PLAN C24 has seeded random
  operation sequences over the fake. Extend to the real transports with a
  chaos proxy: random faults + random client operations, invariants: one
  terminal state, no event after `closed`, all futures complete, no
  growth in native stats. Run nightly with a new seed, keep failing seeds
  as regression cases.
- **14.3 Soak.** Hours-long runs on a real Android and a real iOS device,
  screen off and on, with: RSS, Dart heap, external memory, fd count,
  thread count, wake-ups, battery drain per hour for an idle connection
  with a 25 s heartbeat (publish the number — it is the first question of
  every mobile team). 24 h on a Linux server with 1 000 connections.
- **14.4 Real devices and OS matrix.** Emulators do not reproduce Doze,
  the freezer, OEM background killers, iOS suspension, radio handover or
  NAT64. Plan: a small device set (one old Android 8–10 armv7 or low-RAM
  device, one current Samsung, one Pixel on the newest Android, one
  iPhone on the newest and one on the oldest supported iOS) via Firebase
  Test Lab / BrowserStack for automated smoke runs, plus a manual
  pre-release checklist: airplane toggle, Wi-Fi ↔ LTE, 30 min background,
  Low Power Mode, VPN on/off, captive portal, IPv6-only hotspot.
- **14.5 Browser matrix.** Chrome only today. Add Firefox and WebKit
  (Playwright-driven `dart test -p` custom platforms or a small harness),
  plus one real iOS Safari run per release (5.3). Hidden-tab behaviour
  (5.1) needs a headed browser test.
- **14.6 TLS matrix.** A local CA and a server presenting: expired, not
  yet valid, wrong host, unknown CA, self-signed CA-as-leaf, RSA-only key
  exchange, TLS 1.0/1.1 only, no SAN, long chain, missing intermediate
  (AIA fetch works on Apple/Windows, fails elsewhere), revoked with
  stapled OCSP. Each case × each transport × expected typed error.
  `badssl.com` is useful manually but must not be a CI dependency.
- **14.7 Proxy matrix.** Squid (Basic, no auth, 407 with NTLM offer),
  tinyproxy, mitmproxy (TLS interception with a custom CA), a proxy that
  closes idle tunnels, a proxy that answers 200 then garbage.
- **14.8 Interop matrix.** Autobahn (in CI — D28 says candidate) plus
  real servers in Docker: nginx → echo, Centrifugo, Phoenix, a Socket.IO
  server (to prove the documented non-support fails clearly), uWebSockets,
  gorilla, a Node `ws` server with deflate required.
- **14.9 Upgrade and skew tests.** N−1 `ws_native` with N `ws` and the
  reverse (12.4); a library file from the previous release loaded by path;
  a corrupted library; a library for the wrong architecture.
- **14.10 Performance regression gate.** ROADMAP §6.1 has it. Add the
  *idle* costs (wake-ups, CPU of an idle connection, memory per
  connection) and worst-case FFI call latency (6.6), not only throughput.
- **14.11 Fuzzing in CI.** `cargo fuzz` targets (tape builder, handshake
  parser of the proxy tunnel, config JSON) on a schedule with a corpus in
  the repository; Dart fuzzing of `FrameReader`, `HandshakeResponse`,
  tape reader with a simple mutator under `dart test --tags fuzz`.
- **14.12 Coverage.** Baseline 63.6% VM line coverage (TEST_PLAN). Set a
  floor per layer (protocol ≥ 95%, client ≥ 90%) and track Rust coverage
  (`cargo llvm-cov`) for `net.rs`, `tls.rs`, `lib.rs` — the Rust side is
  where the uncatchable failures live.

## 15. Rust maintenance, supply chain, licensing, binary size

- **15.1 Toolchain and MSRV.** No `rust-toolchain.toml`; CI builds
  releases with floating `stable` (R6); edition 2024. tokio, rustls and
  friends raise MSRV a few times a year; `ring` and `aws-lc-rs` build
  scripts depend on host C toolchains and the NDK version. I:M L:H. Plan:
  commit `rust-toolchain.toml` (same version as `mise.toml`), build
  releases with it, bump deliberately (monthly or on advisories) with the
  full target matrix; record NDK and Xcode versions used per release in
  the release notes.
- **15.2 Advisory response is now the maintainer's job.** Binaries are
  bundled, so a RUSTSEC advisory in rustls, ring, tokio, tungstenite or
  simd-json is fixed for users only when a new `ws_native` is published.
  I:H L:H over the life of the package (rustls/webpki have had several
  advisories). Plan: `cargo deny check` (advisories, licenses, bans,
  sources) and `cargo audit` on every PR and on a daily schedule against
  the released lockfile; Dependabot/Renovate for `Cargo.lock` and Actions;
  a written SLA (for example: assess within 3 days, patch release within
  14 for exploitable issues); the release pipeline must be runnable at any
  time by one command — which means M7 is a 2.0.0 blocker, not polish.
  No `deny.toml` exists yet `[code]`. `[doc]`
  [cargo-deny](https://github.com/EmbarkStudios/cargo-deny),
  [RustSec database](https://rustsec.org/).
- **15.3 Dependency surface.** `serde_json` and `simd-json` are both
  linked; tungstenite brings its own HTTP parsing (`httparse`), SHA-1,
  base64, rand; `rustls-platform-verifier` pulls platform crates
  (`security-framework`, `windows-sys`, and on Android targets JNI crates
  even if unused `[verify]`). Plan: `cargo tree -e normal --target <t>`
  snapshot per target committed and diffed in CI; `cargo deny bans` for
  duplicate versions; `cargo vet` or `cargo crev` if the audience demands
  it; consider parsing the tiny config without `serde_json`.
- **15.4 Upstream health.** tungstenite: maintained but slow on features
  (deflate). `tokio-tungstenite` 0.30 API churn each minor. simd-json:
  small maintainer set, lots of `unsafe`, runtime CPU-feature dispatch
  (check behaviour on armv7 without NEON and on x86_64 emulators without
  AVX2). ring: security-only maintenance (8.7). Plan: a one-page
  "dependency risk register" reviewed each minor release; keep the option
  to drop simd-json for `serde_json` on a target with one feature flag.
- **15.5 SBOM and regulation.** The EU Cyber Resilience Act's reporting
  duties apply from 2026-09-11 and the main obligations from 2027-12-11;
  non-commercial open source is largely outside, but commercial users
  must document their components and will ask for an SBOM and a
  vulnerability-handling policy. Plan: generate a CycloneDX SBOM
  (`cargo cyclonedx`) per release asset and attach it to the GitHub
  release; `SECURITY.md` (11.12). `[doc]`
  [Cyber Resilience Act](https://digital-strategy.ec.europa.eu/en/policies/cyber-resilience-act).
- **15.6 Licences.** The binary statically links code under MIT,
  Apache-2.0, ISC, BSD-3-Clause, the OpenSSL-derived parts of ring,
  Unicode-3.0 (`unicode-ident`), CDLA-Permissive-2.0 and MPL-2.0-derived
  data (`webpki-roots`), Zlib/others depending on versions. Apache-2.0 and
  BSD require the notices to ship with binaries; app stores do not check
  but corporate legal teams do. ROADMAP §4.7 has a one-line item. Plan:
  `cargo about` generates `THIRD_PARTY_NOTICES` in the release workflow;
  append it to `ws_native/LICENSE` so that Flutter's `LicenseRegistry`
  (`showLicensePage`) picks it up automatically; attach the same file to
  release assets for `ws:install` users; `cargo deny licenses` with an
  allow-list so a new copyleft or unknown licence fails CI. If a future
  switch to `aws-lc-rs` happens, re-run (OpenSSL/SSLeay terms).
- **15.7 Binary size budget.** About 2.1–2.3 MB per ABI stripped today
  (D35), 12 targets ≈ 13 MB in the pub archive. No budget is enforced.
  Trends push up: platform verifier, deflate, proxy, pins, mTLS, PQ
  crypto. I:M L:H. Plan: set budgets (for example ≤ 2.5 MB per Android
  ABI stripped, ≤ 20 MB archive), fail CI above them, track with
  `cargo bloat` in the PR summary; levers: `opt-level = "s"` for
  non-hot crates via profile overrides, `panic = "abort"` is *not*
  available (D18 needs unwinding) — measure the unwinding-table cost,
  drop `serde_json`, feature-trim tokio, check `strip` actually removes
  the symbol table on every target, consider not shipping `x64` Apple
  simulators. Publish the per-platform added app size in the README
  (Play/App Store compressed deltas, not file sizes).
- **15.8 Debug symbols.** `strip = true` and no symbol artifacts. Native
  crashes in Play Console, Crashlytics, Sentry and Apple crash logs are
  unsymbolicated addresses in `libws_native`. I:M L:M. Plan: build with
  `split-debuginfo`, upload per-release symbol files (`.so` with debug
  info / Breakpad symbols, dSYM, PDB) to the GitHub release; document how
  to add them to Crashlytics/Sentry/Play (`native-debug-symbols.zip`);
  include the build ID in `metrics`/a `ws_native_build_info()` export.
- **15.9 Panics.** `catch_unwind` at every boundary (D18) turns a panic
  into an error, but the state behind it may be inconsistent and the
  panic message is lost unless forwarded. Plan: a panic hook that sends
  message + location to Dart (`onError`) once and marks the connection
  dead; count panics in native stats; treat any panic in CI as a failure.
- **15.10 Build reproducibility and provenance.** See 11.6/11.7. Also:
  macOS/iOS binaries embed SDK versions; Windows embeds timestamps unless
  `/Brepro`. Aim for "same toolchain + same lockfile → same hash" at least
  on Linux and Android, verified by a second CI job.
- **15.11 Two Rust workspaces and a benchmark crate.** `native/rust`,
  `server`, `benchmark/native` each have their own lockfile; advisories
  in the server or benchmark do not ship but will trigger scanners on the
  repository. Plan: one workspace or at least one `deny.toml` applied to
  all three.
- **15.12 Bus factor.** One maintainer, three transports, twelve binary
  targets, four OS trust stores. The realistic long-term risk is not a
  single bug but the release train stopping. Plan: automate everything in
  M7 so a release is a tag push; keep the pure-Dart transport first-class
  (it is the guaranteed fallback when the native train is late); write
  the runbook (how to cut a security release) in `docs/DEVELOPMENT.md`.

## 16. Observability

- **16.1 No logging hook.** ROADMAP §4.9 lists one; `WebSocketOptions`
  has `onError`, `onStateChange` and counters only `[code]`. Support
  questions ("why did it reconnect?", "which transport?", "why pure
  Dart?") cannot be answered from user logs. I:M L:H. Plan: one
  `onEvent(WebSocketEvent)` callback (sealed, cheap, off by default) with:
  attempt started (redacted URL, attempt, delay), DNS done (addresses,
  ms), TCP connected (family, address, ms), proxy tunnel (ms), TLS done
  (version, suite, resumed, ms), handshake done (status, protocol, ms),
  closed (code, reason, by whom, error kind), heartbeat sent/timeout,
  backpressure start/stop, transport fallback (reason). Rust sends these
  as one more event kind on the existing port.
- **16.2 Metrics gaps.** Present: counters, RTT, last error, transport.
  Missing for production dashboards: time to connect broken down by
  phase, time in each state, reconnect reason histogram, close-code
  histogram, credit stalls (ROADMAP mentions), send-queue high-water
  events, messages dropped by the codec, native memory owned by Dart,
  current backoff delay. Plan: add before 2.0.0 what changes the
  `WebSocketMetrics` shape; keep it a plain data class with additive
  fields.
- **16.3 Correlating with the server.** No connection id. Plan: generate
  a per-attempt id, expose it in events, and optionally send it as a
  header (`X-Request-Id`) on VM transports / query parameter on the web
  (opt-in).
- **16.4 DevTools visibility.** `dart:io` `WebSocket` traffic appears in
  the DevTools Network view; raw sockets and Rust sockets do not.
  Developers will report "no traffic visible". Plan: in debug/profile
  builds post frames (size + truncated preview, opt-in) with
  `developer.postEvent` and a `Timeline` span per connection phase; a
  DevTools extension later if there is demand.
- **16.5 Native diagnostics.** A `ws_native_debug_stats()` export
  (connections, tasks, queued bytes, bytes lent to Dart, panics, threads)
  serves tests (6.4), soak runs (14.3) and bug reports. rustls's
  `logging` feature is enabled `[code]` (`tokio-rustls` features) but no
  logger is installed — either wire `log` to the event stream at a
  debug level or drop the feature (size).
- **16.6 Crash and error reporting integrations.** Document how typed
  exceptions map to Sentry/Crashlytics grouping (stable `toString`
  prefixes, no URLs or addresses in messages — also 11.4).

---

## 17. Top 20 to plan now

Ordered by (impact × likelihood) and by how expensive the item becomes
after 2.0.0 is published. "Freeze" = must be decided before the API or the
defaults are frozen.

| # | Item | Why now | Refs |
|---|---|---|---|
| 1 | **Heartbeat on by default on VM transports, TCP keepalive / `TCP_USER_TIMEOUT`, and ping scheduling moved into Rust** as D15 says | Half-open connections and 60 s LB idle timeouts hit every user; default change is breaking later | R3, R4, 6.7, 9.5, 10.1–10.3 |
| 2 | **App lifecycle and connectivity integration** (suspend/resume API in the core, Flutter observer + path monitor in a companion) with sleep-aware timers | iOS suspension and the Android freezer kill sockets in the background for every mobile app | 2.1, 2.2, 2.5, 3.2, 3.3, 3.9 |
| 3 | **Finish the error and state hierarchy before the freeze**: TLS (with reasons), proxy, local-network/permission, pin mismatch; decide sealed vs `kind` enums | Adding subtypes to sealed classes later is a major version | R8, 8.4, 12.1, 12.2 |
| 4 | **Mixed text + binary client** (`raw()` flavour or public codec hook) | Realtime/voice/AI protocols need it; codecs are private today | 9.1 |
| 5 | **Android trust model**: `trustUserCertificates` option, SPKI pins, "NSC does not apply" documentation; fix the misleading `tls.rs` comment | Managed devices and TLS-inspecting networks fail; pinning is promised and absent | R2, 3.5, 8.2, 8.3 |
| 6 | **Android 17 local network permission and iOS/macOS local network privacy**: typed error via `android_getnetworkblockedreason`, no retry storm, docs | Enforced for target SDK 37; breaks LAN connects with a bare timeout | 2.8, 3.1, 4.3 |
| 7 | **Release pipeline as a 2.0 blocker**: pinned toolchain (`rust-toolchain.toml`), `cargo deny`/`audit` scheduled, SBOM, attestations, symbols, licence notices, one-command security release, `SECURITY.md` | Bundled binaries make every upstream advisory the maintainer's release | R6, 11.6, 11.12, 15.1, 15.2, 15.5, 15.6, 15.8 |
| 8 | **Web-safe `ws_native` import** (conditional export + stub) and a CI job compiling the README snippets for js and wasm | The documented Flutter setup does not compile for the web | R7, 6.1 |
| 9 | **Linux portability**: glibc floor with zigbuild and a CI symbol check, musl build + detection, `scratch`/distroless test, Windows `+crt-static` | Server users are the `dart compile exe` audience; today's floor is glibc 2.35 | R5, 4.5, 4.8–4.10 |
| 10 | **Version-skew policy `ws` ↔ `ws_native` ↔ binary**: tight dependency range, ABI range, no throw on mismatch from `install()`, N−1 CI | A `pub upgrade` must never turn into a startup crash | 12.4, 14.9 |
| 11 | **Web robustness**: visibility/bfcache/pageshow handling, wall-clock liveness, drain check on visibility, Firefox + WebKit + real iOS Safari in the test matrix | Timer throttling stalls `push()` and heartbeats in hidden tabs | R9, 5.1–5.3, 14.5 |
| 12 | **Secret redaction** in every `toString`, exception and event; ticket-auth recipe for the web | Tokens in URLs are unavoidable on the web and currently printed | R14, 5.4, 11.4 |
| 13 | **JSON hardening**: depth limit on all decoders, iterative deep copy, hash index for large objects, read-only results on every transport | Uncatchable stack overflow on the UI isolate; transport-dependent mutability | 6.8, 6.11, 11.3 |
| 14 | **Ecosystem adapters**: `WebSocketChannel` and `package:web_socket` `WebSocket` over `ws`, tested with `web_socket_conformance_tests` | Without them adoption is limited to greenfield code | 13.3, 13.4 |
| 15 | **Server-compat options and defaults**: default `User-Agent`, `maxOutgoingFrameSize`, response headers as multi-map, `Retry-After`/1013 honouring, `maxConnectionAge`, server-driven heartbeat mode, reserved `compression` field | Each is small now and breaking or awkward later | R10, 9.2–9.6, 9.9, 9.10 |
| 16 | **iOS store readiness**: TestFlight upload with `ws_native`, required-reason symbol audit, privacy-manifest instructions, export-compliance note, NAT64 test incl. IPv4 literals | Upload rejection blocks every iOS user at once | R11, 2.7, 2.9, 2.10 |
| 17 | **FFI on the platform thread**: remove the global table mutex from hot calls, bound blocking threads, measure worst-case call latency | Merged UI/platform thread makes any blocking FFI call an ANR/jank source | 2.14, 6.6 |
| 18 | **Observability**: `onEvent` stream with per-phase timings and fallback reasons, extended metrics, `ws_native_debug_stats()`, DevTools timeline events | Needed to support users and to run soak/chaos tests | 16.1–16.5 |
| 19 | **Fault, chaos and soak infrastructure**: toxiproxy/netem jobs, nightly seeded chaos on real transports, device-farm smoke runs, published idle battery/wake-up numbers, TLS and proxy matrices | The failure modes above are not reachable from the current suites | 14.1–14.8 |
| 20 | **Document the security and behaviour contract**: what platform policies do not apply (ATS, NSC, cleartext, system proxy, VPN On Demand, Low Data Mode), rustls strictness vs `dart:io`, per-transport differences table, memory worst case, defaults for mobile; decide the cleartext default and smaller `maxMessageSize` | Sets expectations before users find the differences in production; two defaults are freeze items | R1, R15, 2.4, 2.6, 3.6, 8.1, 11.1, 12.6 |

### Items checked and found adequately covered

For completeness, these were examined and need no new plan beyond what
exists: Happy Eyeballs (D34), fallback when no CA store exists (D35),
session-identity guards against late events (ARCHITECTURE), unhandled
`connect()` errors (D21), re-entrancy (D22), hook resolution order and
checksum (D9, D31), `dart compile exe` path (D30, D32), send-queue hard
limit (D36), backoff reset rule (D14), symbol namespacing of the dylib
(D31), strict handshake validation and header-injection checks (D7).
