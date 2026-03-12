# Python SDK Makefile — no Rust toolchain required (FP-01)
# Delegating pattern (WI-01)

.PHONY: proto-python test lint clean

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
		$(PROTO_DIR)/wheelhouse/v1/*.proto
	@echo "Python proto types generated in $(OUT_DIR)"

## Run Python SDK tests
test:
	uv run pytest tests/ -v

## Lint and type-check
lint:
	uv run pyright wheelhouse/
	uv run pytest tests/ -v --tb=short

## Clean generated files
clean:
	find $(OUT_DIR) -name '*.py' ! -name '__init__.py' -delete
	find . -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
