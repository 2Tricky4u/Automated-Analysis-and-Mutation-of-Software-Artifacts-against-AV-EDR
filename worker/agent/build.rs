fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Paths are relative to the crate root where build.rs lives
    let protos = &[
        "../../controller/proto/common.proto",
        "../../controller/proto/controller.proto",
        "../../controller/proto/worker.proto",
    ];
    let includes = &["../../controller/proto"];

    // file descriptor set for reflection in OUT_DIR
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    let descriptor_path = out_dir.join("edr_descriptor.bin");

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .file_descriptor_set_path(&descriptor_path)
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
