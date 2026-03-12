# Python SDK Makefile — no Rust toolchain required (FP-01)
# Delegating pattern (WI-01)

.PHONY: proto-python test clean

PROTO_DIR := ../../proto
OUT_DIR := wheelhouse/_proto

## Generate Python protobuf types via betterproto
proto-python:
	@echo "Generating Python types from proto files..."
	@if ! command -v uv > /dev/null 2>&1; then \
		echo "ERROR: uv is required for Python SDK development."; \
		echo "Install: curl -LsSf https://astral.sh/uv/install.sh | sh"; \
		exit 1; \
	fi
	env -i HOME="$(HOME)" PATH="$(PATH)" \
		python3 -m grpc_tools.protoc \
		-I$(PROTO_DIR) \
		--python_betterproto_out=$(OUT_DIR) \
		$(PROTO_DIR)/wheelhouse/v1/*.proto 2>/dev/null || \
	echo "NOTE: betterproto codegen requires grpcio-tools. Run: uv pip install grpcio-tools betterproto[compiler]"

## Run Python SDK tests
test:
	uv run pytest

## Clean generated files
clean:
	find $(OUT_DIR) -name '*.py' ! -name '__init__.py' -delete
