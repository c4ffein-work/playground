# Linux or macOS: test, e2e, bindings, swift-test, check.   macOS only: apple, xcode.
# An app is a directory in apps/, with its core in crates/<app>-core.
APPS := $(notdir $(wildcard apps/*))
APP ?= companion
export COMPANION_PY ?= $(CURDIR)/../clones/companion/src/companion.py
export TWOFA_PY ?= $(CURDIR)/../clones/2fa/2fa.py
export PASS_PY ?= $(CURDIR)/../clones/pass/pass.py

help:
	@echo "apps: $(APPS)"
	@echo "make test        Rust tests that need nothing outside this repo"
	@echo "make e2e         Rust tests against the real thing (the ignored ones: see each crate's tests/)"
	@echo "make bindings    generate the Swift bindings of every app into apps/<app>/Package/Generated"
	@echo "make swift-test  every app's Swift package tests (apps/<app>/swift-test.sh; Docker when there is no swift)"
	@echo "make check       fmt, clippy, and all of the above"
	@echo "make scenarios   [root] pass sync: 30 situations on every mix of pass.py and the phone core (about an hour)"
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

# Thirty sync situations on every mix of pass.py and phone-core devices, through a real drop zone.
# Needs root (it starts sshd) and takes about an hour: not part of check.
scenarios:
	cargo build -p pass-core --example pass-device
	apps/pass/scenarios/run.py

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	$(MAKE) test e2e swift-test

apple:
	@set -e; for app in $(APPS); do scripts/build-apple.sh $$app-core $$app; done

xcode: apple
	cd apps/$(APP) && xcodegen generate && open *.xcodeproj

.PHONY: help test e2e bindings swift-test scenarios check apple xcode
