.PHONY: get test

get:
	@dart pub get

test: get
	@dart test --debug --coverage=.coverage --platform chrome,vm

publish:
	@yes | dart pub publish