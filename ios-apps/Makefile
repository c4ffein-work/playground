# Linux or macOS: test, e2e, bindings, swift-test.   macOS only: apple, xcode.
COMPANION_PY ?= $(CURDIR)/../clones/companion/src/companion.py

help:
	@echo "make test        Rust unit tests + TLS pinning tests (no network)"
	@echo "make e2e         Rust client against a real companion server (COMPANION_PY=$(COMPANION_PY))"
	@echo "make bindings    generate the Swift bindings into apps/companion/Package/Generated"
	@echo "make swift-test  Swift package tests against a real companion server (Docker when there is no swift)"
	@echo "make check       all of the above, plus fmt and clippy"
	@echo "make apple       [macOS] build the Rust core as an xcframework"
	@echo "make xcode       [macOS] apple + generate and open apps/companion/Companion.xcodeproj (needs xcodegen)"

test:
	cargo test

e2e:
	COMPANION_PY=$(COMPANION_PY) cargo test -p companion-core --test e2e -- --ignored

bindings:
	scripts/bindings.sh companion-core companion

swift-test: bindings
	COMPANION_PY=$(COMPANION_PY) scripts/swift-test.sh

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	$(MAKE) test e2e swift-test

apple:
	scripts/build-apple.sh companion-core companion

xcode: apple
	cd apps/companion && xcodegen generate && open Companion.xcodeproj

.PHONY: help test e2e bindings swift-test check apple xcode
