fn main() {
    // The SEH shim must be compiled by MSVC: only cl.exe understands
    // __try/__except. `cc` finds it through the VS installation.
    //
    // Same pattern as crates/dcx-engine/build.rs, but a *different* symbol and
    // archive name: dcx-engine also links its own `sehguard`, and two static
    // archives defining `dcx_guard` would collide at link time when both land in
    // the injected DLL.
    cc::Build::new()
        .file("src/seh_guard.c")
        .compile("dcxblurguard");
    println!("cargo:rerun-if-changed=src/seh_guard.c");
}
