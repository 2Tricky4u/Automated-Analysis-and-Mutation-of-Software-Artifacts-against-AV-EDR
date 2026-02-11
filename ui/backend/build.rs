fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Paths are relative to the crate root where build.rs lives
    let protos = &["../../proto/common.proto", "../../proto/controller.proto"];
    let includes = &["../../proto"];

    // Build client only (no server)
    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(protos, includes)?;

    // Rebuild if these change
    for p in protos {
        println!("cargo:rerun-if-changed={p}");
    }
    for inc in includes {
        println!("cargo:rerun-if-changed={inc}");
    }

    Ok(())
}
