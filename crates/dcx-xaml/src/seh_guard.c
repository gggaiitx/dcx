/* SEH guard for dcx-xaml.
 *
 * Rust cannot catch Windows structured exceptions. XAML raises them on invalid
 * markup, and one unwinding through a Rust frame terminates the host process
 * (explorer). This gives the exception a `__try/__except` frame to land in,
 * exactly like the C++ reference's `try { } catch (...) { }`.
 *
 * Identical to crates/dcx-engine/src/seh_guard.c except for the symbol name:
 * both crates are linked into the same DLL and duplicate definitions would be a
 * link error. No `__declspec(dllexport)`: this copy is only ever linked
 * statically, and exporting it just makes the linker emit a useless import
 * library.
 */

#include <windows.h>

int dcx_xaml_guard(void (*fn)(void *), void *arg) {
    __try {
        fn(arg);
        return 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return (int)GetExceptionCode();
    }
}
