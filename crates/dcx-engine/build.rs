fn main() {
    // The exception shim must be compiled by MSVC: only cl.exe understands
    // __try/__except, and only a C++ translation unit can `catch (...)` a
    // C++/WinRT `hresult_error` so that the C++ runtime unwinds properly.
    cc::Build::new()
        .cpp(true)
        .file("src/seh_guard.cpp")
        .flag_if_supported("/EHsc")
        .compile("sehguard");
    println!("cargo:rerun-if-changed=src/seh_guard.cpp");
}
