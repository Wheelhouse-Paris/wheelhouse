# Contributing to Wheelhouse

## Contributor Paths

Wheelhouse supports three contributor paths (WR-A):

### Rust Contributors

```bash
make test-rust    # cargo test --workspace
```

No `uv` or Python toolchain required. Rust proto codegen happens via `cargo build`.

### Python SDK Contributors

```bash
cd sdk/python
make proto-python  # Generate Python types from .proto files
make test          # uv run pytest
```

No Rust toolchain required.

### Full-Stack Contributors

```bash
make test          # Runs test-rust + test-python + test-e2e
```

## Prerequisites

- **Rust**: Version pinned in `rust-toolchain.toml` (installed automatically by `rustup`)
- **Python**: 3.10+ managed via [`uv`](https://docs.astral.sh/uv/)
- **Protobuf**: No system `protoc` required — vendored via `protoc-bin-vendored`

## Cargo.lock Policy

`Cargo.lock` is **committed** to the repository. Wheelhouse ships compiled binaries,
so reproducible builds require a locked dependency graph (KR-02).

When updating dependencies:
1. Run `cargo update` and verify all tests pass
2. Commit the updated `Cargo.lock` in the same PR as the dependency change
3. Do not manually edit `Cargo.lock`

## Editor Setup

Generated Protobuf types require a `cargo build` before they are visible in
`rust-analyzer`. If your editor shows unresolved imports for `wh_proto::*` types,
run `cargo build -p wh-proto` first (MA-01).

## Proto Schema Changes

When modifying `.proto` files:
1. Edit files in `proto/wheelhouse/v1/`
2. Run `make proto-hash` to update `.proto.sha256`
3. Run `cargo test -p wh-proto` to verify
4. If schema is backward-incompatible, update fixtures:
   `cargo test -p wh-proto --test generate_fixtures -- --ignored`
5. Commit `.proto.sha256` and any updated fixture files

## Air-Gapped Builds

For environments without network access (WI-03):

```bash
make vendor        # Vendor all Cargo dependencies to vendor/
```

The `vendor/` directory is not committed. See `.cargo/config.toml` for usage.
