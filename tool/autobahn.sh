#!/usr/bin/env bash
# Autobahn TestSuite (fuzzingserver in Docker) against one transport, then
# the summary (DECISIONS.md D28). Report: benchmark/build/autobahn/index.html
#
# Usage: tool/autobahn.sh [dart|native]
set -euo pipefail
transport="${1:-dart}"
cd "$(cd "$(dirname "$0")/.." && pwd)/benchmark"
mkdir -p build/autobahn
docker rm -f fuzzingserver >/dev/null 2>&1 || true
docker run -d --rm -p 9001:9001 -v "$PWD/autobahn:/config" \
  -v "$PWD/build/autobahn:/reports" --name fuzzingserver \
  crossbario/autobahn-testsuite \
  wstest -m fuzzingserver -s /config/fuzzingserver.json >/dev/null
trap 'docker stop fuzzingserver >/dev/null 2>&1 || true' EXIT
sleep 3
dart pub get >/dev/null
dart run bin/autobahn.dart --transport="$transport"
python3 autobahn/summary.py
