/* SEH guard for dcx-blur.
 *
 * Rust has no way to catch Windows structured exceptions. Several calls this
 * crate must make can raise from native code:
 *
 *   - RoGetActivationFactory / the composable class factory
 *     (IXamlCompositionBrushBaseFactory::CreateInstance),
 *   - XamlReader::Load (used to resolve `{ThemeResource ...}` tint colours,
 *     the reference does the same in XamlBlurBrush's ctor),
 *   - Compositor::CreateEffectFactory and CompositionEffectFactory::CreateBrush.
 *
 * The C++ reference wraps the XamlReader call in try/catch; an escaping SEH
 * exception unwinding through a Rust frame would kill explorer.exe. This shim
 * gives the exception a __try/__except frame to land in, exactly like the
 * reference's `try { ... } catch (...)`.
 *
 * Symbol name is deliberately crate-unique (dcx-engine links its own, separate
 * shim with symbol `dcx_guard`).
 */

#include <windows.h>

__declspec(dllexport) int dcx_blur_guard(void (*fn)(void *), void *arg) {
    __try {
        fn(arg);
        return 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return (int)GetExceptionCode();
    }
}
