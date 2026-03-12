use std::io::Result;

fn main() -> Result<()> {
    // protoc-bin-vendored provides a bundled protoc binary
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("protoc binary not found");
    std::env::set_var("PROTOC", protoc);

    prost_build::compile_protos(
        &[
            "../../proto/wheelhouse/v1/system.proto",
            "../../proto/wheelhouse/v1/skills.proto",
            "../../proto/wheelhouse/v1/core.proto",
        ],
        &["../../proto"],
    )?;
    Ok(())
}
