use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use vendored protoc binary — no system protoc required (SA-01)
    let protoc = protoc_bin_vendored::protoc_bin_path()
        .map_err(|e| format!("Failed to find vendored protoc: {e}"))?;

    let proto_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("proto");

    let proto_files = &[
        proto_root.join("wheelhouse/v1/core.proto"),
        proto_root.join("wheelhouse/v1/skills.proto"),
        proto_root.join("wheelhouse/v1/system.proto"),
        proto_root.join("wheelhouse/v1/stream.proto"),
    ];

    // Verify all proto files exist
    for f in proto_files {
        if !f.exists() {
            return Err(format!("Proto file not found: {}", f.display()).into());
        }
    }

    prost_build::Config::new()
        .protoc_executable(protoc)
        .compile_protos(proto_files, &[proto_root.as_path()])?;

    Ok(())
}
