# SDK Python — proto codegen and test targets

PROTO_DIR := ../../proto
OUT_DIR := wheelhouse/_proto

.PHONY: proto-python test clean

proto-python:
	@echo "Generating Python protobuf types with betterproto..."
	@mkdir -p $(OUT_DIR)
	uv run --extra dev python -m grpc_tools.protoc \
		-I$(PROTO_DIR) \
		--python_betterproto_out=$(OUT_DIR) \
		$(PROTO_DIR)/*.proto
	@echo "Generated Python proto types in $(OUT_DIR)"

test:
	uv run --extra dev pytest -v

clean:
	rm -rf $(OUT_DIR)
