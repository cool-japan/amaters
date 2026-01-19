fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use tonic-prost-build for proto compilation with service generation
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(
            &[
                "protocol/aql.proto",
                "protocol/query.proto",
                "protocol/types.proto",
                "protocol/errors.proto",
            ],
            &["protocol"],
        )?;

    println!("cargo:rerun-if-changed=protocol/");
    Ok(())
}
