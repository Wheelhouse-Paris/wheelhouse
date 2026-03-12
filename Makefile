# Wheelhouse — delegating Makefile
# Per architecture: sdk/*/Makefile pattern (WI-01)

.PHONY: proto proto-rust proto-python test test-rust test-python

# Proto targets
proto: proto-rust proto-python

proto-rust:
	@echo "Proto Rust codegen (prost-build via build.rs)"
	cargo build -p wh-proto

proto-python:
	@echo "Proto Python codegen (betterproto)"
	cd sdk/python && $(MAKE) proto-python

# Test targets
test: test-rust test-python

test-rust:
	cargo test --workspace

test-python:
	cd sdk/python && uv run --extra dev pytest -v
