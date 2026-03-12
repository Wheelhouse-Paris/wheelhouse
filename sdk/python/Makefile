.PHONY: proto-python test lint clean

PROTO_DIR := ../../proto
OUT_DIR := wheelhouse/_proto

# Generate Python types from Protobuf using betterproto
proto-python:
	@echo "Generating Python types from Protobuf..."
	python -m grpc_tools.protoc \
		-I $(PROTO_DIR) \
		--python_betterproto_out=$(OUT_DIR) \
		$(PROTO_DIR)/wheelhouse/v1/*.proto
	@echo "Python proto types generated in $(OUT_DIR)"

test:
	uv run pytest tests/ -v

lint:
	uv run pyright wheelhouse/
	uv run pytest tests/ -v --tb=short

clean:
	rm -rf $(OUT_DIR)/*.py $(OUT_DIR)/*.pyi
	find . -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
