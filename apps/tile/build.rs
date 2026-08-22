fn main() {
    // Build provenance (see `src/build_kind.rs`). Re-emitting the variable
    // makes it a recorded input of the crate, so a build that flips it always
    // recompiles instead of reusing a cached artifact of the other kind.
    println!("cargo:rerun-if-env-changed=TILE_BUILD_KIND");
    println!(
        "cargo:rustc-env=TILE_BUILD_KIND={}",
        std::env::var("TILE_BUILD_KIND").unwrap_or_default()
    );

    tauri_build::build();
}
