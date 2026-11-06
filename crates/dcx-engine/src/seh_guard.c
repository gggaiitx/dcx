/* SEH guard.
 *
 * Rust has no way to catch Windows structured exceptions. `XamlReader::Load`
 * raises one on invalid markup, and letting it unwind through a Rust frame
 * terminates the host process (observed twice: explorer restarting mid-apply).
 *
 * This shim gives us a `__try/__except` frame the exception can land in, which
 * is exactly what the C++ reference gets from `try { } catch (...) { }`.
 */

#include <windows.h>

__declspec(dllexport) int dcx_guard(void (*fn)(void *), void *arg) {
    __try {
        fn(arg);
        return 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return (int)GetExceptionCode();
    }
}
