.PHONY: get test publish server-up server-down

get:
	@dart pub get

test: get
	@dart test --debug --coverage=.coverage --platform chrome,vm

publish:
	@yes | dart pub publish

server-up:
	@docker compose -f server/docker-compose.yml up -d --remove-orphans

server-down:
	@docker compose -f server/docker-compose.yml down --remove-orphans