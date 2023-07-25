.PHONY: format get test publish deploy server-up server-down coverage analyze check pana

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

server-up:
	@docker compose -f server/docker-compose.yml up -d --remove-orphans

server-down:
	@docker compose -f server/docker-compose.yml down --remove-orphans

# dart run coverage:test_with_coverage -fb -o coverage -- --concurrency=6 --platform chrome,vm --coverage=./coverage --reporter=expanded test/ws_test.dart
coverage: get
	@dart test --concurrency=6 --platform chrome,vm --coverage=coverage test/
	@dart run coverage:format_coverage --lcov --in=coverage --out=coverage/lcov.info --packages=.packages --report-on=lib
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