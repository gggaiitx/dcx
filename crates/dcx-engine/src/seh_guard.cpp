/* Exception guard shim.
 *
 * Rust cannot catch Windows exceptions. XAML raises two completely different
 * kinds:
 *
 *  - hardware/SEH exceptions (e.g. an access violation), caught by __except;
 *  - C++ exceptions (C++/WinRT's `winrt::hresult_error`, raised as SEH code
 *    0xE06D7363), which must be caught by a C++ `catch` so that the C++ runtime
 *    unwinds properly. Catching those with `__except(EXECUTE_HANDLER)` swallows
 *    the throw without unwinding, and the process crashes later (observed).
 *
 * The two are split into two functions because MSVC forbids __try in a function
 * that needs C++ object unwinding (C2712).
 */

#include <windows.h>

/* C++ exceptions land here. -1 means "C++ exception", which is what the
 * reference implementation's `try { ... } catch (winrt::hresult_error&)` does. */
static int dcx_guard_cpp(void (*fn)(void*), void* arg) {
    try {
        fn(arg);
        return 0;
    } catch (...) {
        return -1;
    }
}

extern "C" __declspec(dllexport) int dcx_guard(void (*fn)(void*), void* arg) {
    __try {
        return dcx_guard_cpp(fn, arg);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return (int)GetExceptionCode();
    }
}
