#!/usr/bin/env bash
# Runs device_test/ (integration tests) on a connected device against the
# Rust test server on this machine (DECISIONS.md D45).
#
# Usage: tool/device_test.sh <device-id> [--ca DIR] [--user-ca] [test file]
#   device-id  from `flutter devices`
#   --ca DIR   directory with the test CA (ca.pem, ca.key); created when
#              missing. Default: build/device_ca.
#   --user-ca  the CA is installed as a user CA on the device (Android:
#              Settings > Security > Install a certificate > CA certificate;
#              push it with `adb push DIR/ca.crt /sdcard/Download/`).
#   test file  default: integration_test/tls_test.dart
#
# Android reaches the server through `adb reverse` (localhost); other
# devices through this machine's LAN address, which the leaf certificate
# then names.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
device="${1:?usage: tool/device_test.sh <device-id> [--ca DIR] [--user-ca] [test file]}"
shift
ca="$root/build/device_ca"
user_ca=false
file=integration_test/tls_test.dart
while (($#)); do
  case "$1" in
    --ca) ca="$2"; shift 2 ;;
    --user-ca) user_ca=true; shift ;;
    *) file="$1"; shift ;;
  esac
done
port=9443

server="$root/server/target/release/ws_bench_server"
[[ -x "$server" ]] || (cd "$root/server" && cargo build --release)

mkdir -p "$ca"
if [[ ! -f "$ca/ca.pem" ]]; then
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout "$ca/ca.key" -out "$ca/ca.pem" -days 30 \
    -subj "/CN=ws test CA (delete me)" \
    -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
  openssl x509 -in "$ca/ca.pem" -outform der -out "$ca/ca.crt"
fi

if adb -s "$device" get-state >/dev/null 2>&1; then
  host=localhost
  adb -s "$device" reverse "tcp:$port" "tcp:$port" >/dev/null
  san="DNS:localhost,IP:127.0.0.1"
else
  host="$(ipconfig getifaddr en0 2>/dev/null || hostname -I | awk '{print $1}')"
  san="DNS:localhost,IP:127.0.0.1,IP:$host"
fi

# A fresh leaf for this host, issued by the CA.
work="$(mktemp -d)"
trap 'rm -rf "$work"; kill "${pid:-0}" 2>/dev/null || true' EXIT
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout "$work/leaf.key" -out "$work/leaf.csr" -subj "/CN=localhost" 2>/dev/null
printf 'subjectAltName=%s\nextendedKeyUsage=serverAuth\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature\n' \
  "$san" >"$work/leaf.ext"
openssl x509 -req -in "$work/leaf.csr" -CA "$ca/ca.pem" -CAkey "$ca/ca.key" \
  -set_serial "0x$(openssl rand -hex 8)" -days 1 -extfile "$work/leaf.ext" \
  -out "$work/leaf.pem" 2>/dev/null
pin="$(openssl x509 -in "$work/leaf.pem" -pubkey -noout | openssl pkey -pubin -outform der |
  openssl dgst -sha256 -binary | base64)"

"$server" --host 0.0.0.0 --port 0 --tls-port "$port" --no-ipv6 \
  --tls-cert "$work/leaf.pem" --tls-key "$work/leaf.key" &
pid=$!
sleep 1

cd "$root/device_test"
flutter test "$file" -d "$device" \
  --dart-define=WS_HOST="$host" \
  --dart-define=WSS_PORT="$port" \
  --dart-define=CA_PEM="$(base64 <"$ca/ca.pem" | tr -d '\n')" \
  --dart-define=LEAF_PIN="$pin" \
  --dart-define=USER_CA="$user_ca"
