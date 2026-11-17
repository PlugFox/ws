#!/usr/bin/env bash
# Autobahn TestSuite (fuzzingserver in Docker) against one transport, then
# the summary (DECISIONS.md D28). Report: benchmark/build/autobahn/index.html
#
# Usage: tool/autobahn.sh [dart|native]
set -euo pipefail
transport="${1:-dart}"
# The image D28 was measured with, pinned by digest.
image=crossbario/autobahn-testsuite@sha256:519915fb568b04c9383f70a1c405ae3ff44ab9e35835b085239c258b6fac3074
cd "$(cd "$(dirname "$0")/.." && pwd)/benchmark"
mkdir -p build/autobahn
docker rm -f fuzzingserver >/dev/null 2>&1 || true
docker run -d --rm -p 9001:9001 -v "$PWD/autobahn:/config" \
  -v "$PWD/build/autobahn:/reports" --name fuzzingserver \
  "$image" \
  wstest -m fuzzingserver -s /config/fuzzingserver.json >/dev/null
trap 'docker stop fuzzingserver >/dev/null 2>&1 || true' EXIT
# Wait for the port instead of a fixed sleep (at most 20 s).
for _ in $(seq 100); do
  (exec 3<>/dev/tcp/127.0.0.1/9001) 2>/dev/null && break
  sleep 0.2
done
dart pub get >/dev/null
dart run bin/autobahn.dart --transport="$transport"
python3 autobahn/summary.py
