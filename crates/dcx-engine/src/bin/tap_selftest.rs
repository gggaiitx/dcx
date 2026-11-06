//! Local lifecycle test for the TAP COM objects.
//!
//! Exercises the parts that are easy to get wrong — vtable slot order, refcount
//! balance, QueryInterface answers — without touching any live system process.
//! A real XAML site is not needed: `GetSite` is expected to fail on an unsited
//! object, and that failure is itself the assertion.

use core::ffi::c_void;
use dcx_engine::com::*;

type CreateInstanceFn =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> HRESULT;

unsafe fn slot_fn(this: *mut c_void, index: usize) -> *const c_void {
    let vtbl = *(this as *const *const *const c_void);
    *vtbl.add(index)
}

fn main() {
    unsafe {
        println!("== DCX TAP lifecycle test ==");

        // 1. factory from DllGetClassObject
        let mut factory: *mut c_void = core::ptr::null_mut();
        let hr = dcx_engine::tap::DllGetClassObject(
            &CLSID_DCXTAP as *const Guid,
            &IID_ICLASSFACTORY as *const Guid,
            &mut factory,
        );
        println!("DllGetClassObject(IClassFactory) -> 0x{:08X}", hr as u32);
        assert_eq!(hr, S_OK);
        assert!(!factory.is_null());

        // 2. wrong CLSID must be rejected
        let bogus = Guid::new(0x1111_2222, 0x3333, 0x4444, [0; 8]);
        let mut nope: *mut c_void = core::ptr::null_mut();
        let hr_bad = dcx_engine::tap::DllGetClassObject(
            &bogus as *const Guid,
            &IID_ICLASSFACTORY as *const Guid,
            &mut nope,
        );
        println!("DllGetClassObject(bogus)        -> 0x{:08X}", hr_bad as u32);
        assert_eq!(hr_bad, CLASS_E_CLASSNOTAVAILABLE);

        // 3. create the TAP, ask for IObjectWithSite
        let create: CreateInstanceFn = core::mem::transmute(slot_fn(factory, 3));
        let mut tap: *mut c_void = core::ptr::null_mut();
        let hr = create(
            factory,
            core::ptr::null_mut(),
            &IID_IOBJECTWITHSITE as *const Guid,
            &mut tap,
        );
        println!("CreateInstance(IObjectWithSite) -> 0x{:08X}", hr as u32);
        assert_eq!(hr, S_OK);
        assert!(!tap.is_null());

        // 4. unsited GetSite must fail cleanly (not crash)
        type GetSiteFn =
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT;
        let get_site: GetSiteFn = core::mem::transmute(slot_fn(tap, 4));
        let mut site: *mut c_void = 1 as *mut c_void;
        let hr = get_site(tap, &IID_IUNKNOWN as *const Guid, &mut site);
        println!("unsited GetSite(IUnknown)       -> 0x{:08X}", hr as u32);
        assert_eq!(hr, E_FAIL);
        assert!(site.is_null());

        // 5. QueryInterface answers
        type QiFn =
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT;
        let qi: QiFn = core::mem::transmute(slot_fn(tap, 0));
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = qi(tap, &IID_IUNKNOWN as *const Guid, &mut out);
        println!("QI(IUnknown)                    -> 0x{:08X}", hr as u32);
        assert_eq!(hr, S_OK);

        let mut out2: *mut c_void = core::ptr::null_mut();
        let hr = qi(tap, &IID_IXAMLDIAGNOSTICS as *const Guid, &mut out2);
        println!("QI(IXamlDiagnostics)            -> 0x{:08X}", hr as u32);
        assert_eq!(hr, E_NOINTERFACE);

        // 6. refcount balance.
        // After CreateInstance: 1 (creation ref handed to us).
        // QI in step 5 added one -> 2.
        // Releasing the QI ref must land on 1, releasing the creation ref on 0.
        type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
        let release: ReleaseFn = core::mem::transmute(slot_fn(tap, 2));
        let r1 = release(out);
        let r2 = release(tap);
        println!("Release x2                      -> {r1}, {r2} (expect 1 then 0)");
        assert_eq!(r1, 1);
        assert_eq!(r2, 0);

        println!("\nall assertions passed");
    }
}
