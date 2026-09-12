use std::env;
use std::path::PathBuf;

fn main() {
    // Same pattern as cortex-m-rt's link.x: pass the defmt KEEP script
    // additively via a build-script link arg. Using `[build] rustflags` in
    // .cargo/config.toml does NOT work here -- cargo-qtest injects its own
    // linker fixup through `--config target.<triple>.rustflags`, which
    // overrides build.rustflags entirely.
    let dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rustc-link-search={}", dir.display());
    println!("cargo:rustc-link-arg=-Tdefmt.x");
    println!("cargo:rerun-if-changed=defmt.x");
}
