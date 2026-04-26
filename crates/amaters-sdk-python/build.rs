/// Build script for amaters-sdk-python.
///
/// When the `extension-module` feature is active, pyo3-ffi's build script suppresses
/// Python link directives because `.so` Python extension modules receive Python symbols
/// from the host interpreter at runtime (via dlopen). However, test binaries compiled
/// from the `lib` crate-type still need Python explicitly linked.
///
/// This build script re-emits the Python link directives when `extension-module` is
/// enabled, so that `cargo nextest run --all-features` and `cargo test --all-features`
/// produce valid test executables on all platforms.
fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_EXTENSION_MODULE");
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    println!("cargo:rerun-if-env-changed=PYO3_CONFIG_FILE");

    // Only needed when the extension-module feature is active.
    // Without this feature, pyo3-ffi already emits the correct link directives.
    if std::env::var("CARGO_FEATURE_EXTENSION_MODULE").is_ok() {
        let config = pyo3_build_config::get();

        // Emit the library to link against (e.g. `python3.11`)
        if let Some(lib_name) = &config.lib_name {
            let link_model = if config.shared { "" } else { "static=" };
            println!("cargo:rustc-link-lib={link_model}{lib_name}");
        }

        // Emit the directory containing the Python shared library
        if let Some(lib_dir) = &config.lib_dir {
            println!("cargo:rustc-link-search=native={lib_dir}");
        }
    }
}
