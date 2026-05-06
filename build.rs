fn main() {
    // Only generate the C header when the `ffi` feature is active.
    // This avoids running cbindgen on WASM builds where there is no C target.
    #[cfg(feature = "ffi")]
    {
        let crate_dir =
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo");

        // Create the include/ directory if it doesn't exist
        std::fs::create_dir_all(format!("{crate_dir}/include"))
            .expect("Failed to create include/ directory");

        let config = cbindgen::Config::from_file(format!("{crate_dir}/cbindgen.toml"))
            .expect("Failed to load cbindgen.toml");

        cbindgen::Builder::new()
            .with_crate(&crate_dir)
            .with_config(config)
            .generate()
            .expect("cbindgen failed to generate header")
            .write_to_file(format!("{crate_dir}/include/epub_rs.h"));

        println!("cargo:rerun-if-changed=src/ffi.rs");
        println!("cargo:rerun-if-changed=cbindgen.toml");
    }

    // Always re-run if build.rs changes
    println!("cargo:rerun-if-changed=build.rs");
}
