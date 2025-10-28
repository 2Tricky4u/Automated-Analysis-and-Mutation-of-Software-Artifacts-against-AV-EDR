fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Paths are relative to the crate root where build.rs lives
    let protos = &[
        "proto/common.proto",
        "proto/controller.proto",
        "proto/worker.proto",
    ];
    let includes = &["proto"];

    tonic_prost_build::configure()
        .build_server(true)
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
