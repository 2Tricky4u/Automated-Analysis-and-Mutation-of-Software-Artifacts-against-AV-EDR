/// Build script for the UI backend crate.
///
/// Compiles the Controller and Common protobuf definitions into Rust client
/// stubs using tonic-prost-build. Only client code is generated — the UI
/// backend never hosts a gRPC server.
fn main() -> Result<(), Box<dyn std::error::Error>> {
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
