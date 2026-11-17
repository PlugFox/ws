// Linker flags for the cdylib. Set here rather than through
// CARGO_TARGET_<TRIPLE>_RUSTFLAGS from the Dart hook: a user's
// CARGO_ENCODED_RUSTFLAGS would silently override those (DECISIONS.md D9).
fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match os.as_str() {
        // Google Play requires 16 KB page alignment.
        "android" => println!("cargo:rustc-cdylib-link-arg=-Wl,-z,max-page-size=16384"),
        // Room for install_name_tool, which the Dart tooling runs on the
        // dylib, and a relocatable install name.
        "macos" | "ios" => {
            println!("cargo:rustc-cdylib-link-arg=-Wl,-headerpad_max_install_names");
            println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libws_native.dylib");
        }
        // No libgcc_s.so.1 at run time: minimal images (FROM scratch,
        // distroless) do not have it (distribution.md F2).
        "linux" => println!("cargo:rustc-cdylib-link-arg=-static-libgcc"),
        _ => {}
    }
    println!("cargo:rerun-if-changed=build.rs");
}
