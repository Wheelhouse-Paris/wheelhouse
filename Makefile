# Wheelhouse Makefile — delegating pattern
# Rust contributors: `make test-rust` (no uv required)
# Python contributors: `make test-python` (no rustup required)
# Full test: `make test`

.PHONY: proto-rust proto-python proto proto-check \
        test-rust test-python test-e2e test \
        check build install \
        vendor clean

# ──────────────────────────────────────────────
# Install config (override via env or make INSTALL_PREFIX=...)
# ──────────────────────────────────────────────
INSTALL_PREFIX ?= /opt/homebrew/bin

PROTO_DIR := proto
PROTO_FILES := $(shell find $(PROTO_DIR) -name '*.proto' 2>/dev/null)
SHA256_FILE := .proto.sha256
PROTOC_SHA256_FILE := .protoc.sha256

# ──────────────────────────────────────────────
# Proto targets
# ──────────────────────────────────────────────

## Rust proto codegen — no uv required (FP-01)
proto-rust:
	env -i HOME="$(HOME)" PATH="$(PATH)" cargo build -p wh-proto

## Python proto codegen — no rustup required (FP-01)
proto-python:
	$(MAKE) -C sdk/python proto-python

## All proto codegen
proto: proto-rust proto-python

## CI gate — verifies .proto.sha256, no side effects (DM-01)
proto-check:
	@echo "Checking proto file hashes..."
	@find $(PROTO_DIR) -name '*.proto' -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1 > /tmp/proto-sha256-check
	@if [ ! -f "$(SHA256_FILE)" ]; then \
		echo "FAIL: $(SHA256_FILE) not found. Run 'make proto-hash' first."; \
		exit 1; \
	fi
	@if ! diff -q /tmp/proto-sha256-check "$(SHA256_FILE)" > /dev/null 2>&1; then \
		echo "FAIL: Proto files have changed. Run 'make proto-hash' to update."; \
		echo "  Expected: $$(cat $(SHA256_FILE))"; \
		echo "  Got:      $$(cat /tmp/proto-sha256-check)"; \
		exit 1; \
	fi
	@echo "OK: Proto hashes match."
	@echo "Checking proto fixture files..."
	@if [ ! -d "tests/fixtures/proto" ] || [ -z "$$(ls tests/fixtures/proto/*.bin 2>/dev/null)" ]; then \
		echo "FAIL: Proto fixture files missing (NFR-E1). Run fixture generation."; \
		exit 1; \
	fi
	@echo "OK: Proto fixtures present."
	@echo "Running backward-compat tests..."
	@env -i HOME="$(HOME)" PATH="$(PATH)" cargo test -p wh-proto --test proto_compat 2>&1 || \
		(echo "FAIL: Proto backward-compat tests failed."; exit 1)
	@echo "OK: Proto check passed."

## Generate .proto.sha256 hash file
proto-hash:
	@find $(PROTO_DIR) -name '*.proto' -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1 > "$(SHA256_FILE)"
	@echo "Updated $(SHA256_FILE): $$(cat $(SHA256_FILE))"

## Generate .protoc.sha256 hash file
protoc-hash:
	@env -i HOME="$(HOME)" PATH="$(PATH)" cargo build -p wh-proto 2>/dev/null
	@find target -path '*/build/protoc-bin-vendored-*/out/bin/protoc' -print0 2>/dev/null | head -1 | xargs -0 shasum -a 256 | cut -d' ' -f1 > "$(PROTOC_SHA256_FILE)"
	@echo "Updated $(PROTOC_SHA256_FILE): $$(cat $(PROTOC_SHA256_FILE))"

# ──────────────────────────────────────────────
# Test targets
# ──────────────────────────────────────────────

## Rust tests — cargo test --workspace
test-rust:
	cargo test --workspace

## Python SDK tests
test-python:
	$(MAKE) -C sdk/python test

## End-to-end tests
test-e2e:
	@echo "E2E tests not yet configured (Story 1.2+)"

## Full test suite (AC#1: all Rust unit tests AND Python SDK tests)
test: test-rust test-python

# ──────────────────────────────────────────────
# Build targets
# ──────────────────────────────────────────────

CARGO ?= $(HOME)/.cargo/bin/cargo

.DEFAULT_GOAL := build

## Run the same checks as CI (fmt + clippy)
check:
	$(CARGO) fmt --check
	$(CARGO) clippy --workspace -- -D warnings

## Build release binaries (default target)
build:
	$(CARGO) build --release -p wh-cli -p wh-telegram

## Copy built binaries to INSTALL_PREFIX (run with sudo if needed)
install:
	cp target/release/wh $(INSTALL_PREFIX)/wh
	cp target/release/wh-telegram $(INSTALL_PREFIX)/wh-telegram
	@echo "Installed to $(INSTALL_PREFIX)"

## Vendor dependencies for air-gapped builds (WI-03)
vendor:
	cargo vendor vendor/
	@echo "Vendored dependencies to vendor/"
	@echo "To use: set [source.crates-io] replacement in .cargo/config.toml"

## Clean build artifacts
clean:
	cargo clean
	rm -rf vendor/
