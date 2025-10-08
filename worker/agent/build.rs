fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("../../controller/proto/edr.proto")?;
    Ok(())
}
