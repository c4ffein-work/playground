# Linux or macOS: test, e2e, bindings, swift-test, check.   macOS only: apple, xcode.
# An app is a directory in apps/, with its core in crates/<app>-core.
APPS := $(notdir $(wildcard apps/*))
APP ?= companion
export COMPANION_PY ?= $(CURDIR)/../clones/companion/src/companion.py

help:
	@echo "apps: $(APPS)"
	@echo "make test        Rust tests that need nothing outside this repo"
	@echo "make e2e         Rust tests against the real thing (the ignored ones: see each crate's tests/)"
	@echo "make bindings    generate the Swift bindings of every app into apps/<app>/Package/Generated"
	@echo "make swift-test  every app's Swift package tests (apps/<app>/swift-test.sh; Docker when there is no swift)"
	@echo "make check       fmt, clippy, and all of the above"
	@echo "make apple       [macOS] build every Rust core as an xcframework"
	@echo "make xcode       [macOS] apple + generate and open apps/\$$APP/*.xcodeproj (needs xcodegen; APP=$(APP))"

test:
	cargo test

e2e:
	cargo test -- --ignored

bindings:
	@set -e; for app in $(APPS); do scripts/bindings.sh $$app-core $$app; done

swift-test: bindings
	@set -e; for app in $(APPS); do echo "== $$app"; apps/$$app/swift-test.sh; done

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	$(MAKE) test e2e swift-test

apple:
	@set -e; for app in $(APPS); do scripts/build-apple.sh $$app-core $$app; done

xcode: apple
	cd apps/$(APP) && xcodegen generate && open *.xcodeproj

.PHONY: help test e2e bindings swift-test check apple xcode
