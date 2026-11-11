.PHONY: format get test publish deploy server bench bench-quick coverage analyze check pana

format:
	@echo "Formatting the code"
	@dart format -l 80 --fix .
	@dart fix --apply .

get:
	@dart pub get

test: get
	@dart test --debug --coverage=.coverage --platform chrome,vm

publish:
	@yes | dart pub publish

deploy: publish

# Rust WebSocket server for tests, benchmarks and examples.
server:
	@cd server && cargo run --release

# Full benchmark matrix (AOT, 3 repeats). Starts and stops the server itself.
bench:
	@cd server && cargo build --release
	@cd benchmark && dart pub get && mkdir -p build && dart build cli -t bin/bench.dart -o build/cli
	@cd benchmark && dart run bin/matrix.dart --repeat=3 --server=../server/target/release/ws_bench_server | tee build/matrix.md

bench-quick:
	@cd server && cargo build --release
	@cd benchmark && dart pub get && mkdir -p build && dart build cli -t bin/bench.dart -o build/cli
	@cd benchmark && dart run bin/matrix.dart --quick --server=../server/target/release/ws_bench_server

# dart run coverage:test_with_coverage -fb -o coverage -- --concurrency=6 --platform chrome,vm --coverage=./coverage --reporter=expanded test/ws_test.dart
coverage: get
	@dart test --concurrency=6 --platform chrome,vm --coverage=coverage test/
	@dart run coverage:format_coverage --lcov --in=coverage --out=coverage/lcov.info --report-on=lib
#	@mv coverage/lcov.info coverage/lcov.base.info
#	@lcov -r coverage/lcov.base.info -o coverage/lcov.base.info "lib/**.freezed.dart" "lib/**.g.dart"
#	@mv coverage/lcov.base.info coverage/lcov.info
	@lcov --list coverage/lcov.info
	@genhtml -o coverage coverage/lcov.info

analyze: get format
	@echo "Analyze the code"
	@dart analyze --fatal-infos --fatal-warnings

check: analyze
	@dart pub publish --dry-run
	@dart pub global activate pana
	@pana --json --no-warning --line-length 80 > log.pana.json

pana: check