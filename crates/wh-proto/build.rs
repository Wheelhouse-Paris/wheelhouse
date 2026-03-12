use std::io::Result;

fn main() -> Result<()> {
    prost_build::compile_protos(
        &["../../proto/wheelhouse/v1/system.proto"],
        &["../../proto"],
    )?;
    Ok(())
}
