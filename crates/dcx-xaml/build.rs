fn main() {
    // `__try/__except` is a compiler intrinsic: only MSVC's cl.exe can build it,
    // which the `cc` crate locates through the Visual Studio installation.
    //
    // A *copy* of dcx-engine's shim is compiled here (same C source, different
    // exported symbol `dcx_xaml_guard`) instead of linking the engine's
    // `sehguard` library: both crates end up in the same injected DLL, so a
    // shared symbol name would be a duplicate definition at link time.
    cc::Build::new()
        .file("src/seh_guard.c")
        .compile("xamlguard");
    println!("cargo:rerun-if-changed=src/seh_guard.c");
}
