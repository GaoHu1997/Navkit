fn main() {
    assert!(cfg!(target_os = "windows"), "Simulation proxy requires Windows ODBC manager");
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // Forwarders are appended by the link wrapper *after* rustc assembles symbol roots.
    // Supplying /DEF or /EXPORT to rustc makes it root nonexistent local symbols.
    assert!(manifest.join("proxy.def").is_file());
    println!("cargo:rerun-if-changed=proxy.def");
}
