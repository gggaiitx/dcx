//! Minimal COM plumbing: GUIDs, HRESULTs, IUnknown vtable, BSTR.
//!
//! Hand-rolled rather than using the `windows` crate because the engine only
//! needs four interfaces and the build must stay fast enough to iterate on.

#![allow(non_snake_case)]

use core::ffi::c_void;

pub type HRESULT = i32;
pub type ULONG = u32;
pub type BSTR = *mut u16;

pub const S_OK: HRESULT = 0;
pub const S_FALSE: HRESULT = 1;
pub const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as i32;
pub const E_FAIL: HRESULT = 0x8000_4005u32 as i32;
pub const E_POINTER: HRESULT = 0x8000_4003u32 as i32;
pub const CLASS_E_NOAGGREGATION: HRESULT = 0x8004_0110u32 as i32;
pub const CLASS_E_CLASSNOTAVAILABLE: HRESULT = 0x8004_0111u32 as i32;
pub const ERROR_NOT_FOUND_HR: HRESULT = 0x8007_0490u32 as i32; // HRESULT_FROM_WIN32(1168)

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Self {
        Self {
            data1: d1,
            data2: d2,
            data3: d3,
            data4: d4,
        }
    }
}

pub const IID_IUNKNOWN: Guid = Guid::new(0x0000_0000, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_ICLASSFACTORY: Guid = Guid::new(0x0000_0001, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IOBJECTWITHSITE: Guid = Guid::new(
    0xFC48_01A3,
    0x2BA9,
    0x11CF,
    [0xA2, 0x29, 0x00, 0xAA, 0x00, 0x3D, 0x73, 0x52],
);

/// Our TAP class. Any GUID works as long as `InitializeXamlDiagnosticsEx`
/// receives the same one; this is not registered anywhere.
pub const CLSID_DCXTAP: Guid = Guid::new(
    0x7C1D_5E42,
    0x9A3B,
    0x4F18,
    [0x8B, 0x62, 0x0D, 0x77, 0xE5, 0x31, 0xA4, 0xC9],
);

// --- XAML diagnostics, from xamlom.h ---------------------------------------

pub const IID_IXAMLDIAGNOSTICS: Guid = Guid::new(
    0x18C9_E2B6,
    0x3F43,
    0x4116,
    [0x9F, 0x2B, 0xFF, 0x93, 0x5D, 0x77, 0x70, 0xD2],
);
pub const IID_IVISUALTREESERVICE3: Guid = Guid::new(
    0x0E79_C6E0,
    0x85A0,
    0x4BE8,
    [0xB4, 0x1A, 0x65, 0x5C, 0xF1, 0xFD, 0x19, 0xBD],
);
pub const IID_IVISUALTREESERVICECALLBACK2: Guid = Guid::new(
    0xBAD9_EB88,
    0xAE77,
    0x4397,
    [0xB9, 0x48, 0x5F, 0xA2, 0xDB, 0x0A, 0x19, 0xEA],
);

// --- IUnknown ---------------------------------------------------------------

#[repr(C)]
pub struct IUnknownVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> ULONG,
    pub Release: unsafe extern "system" fn(*mut c_void) -> ULONG,
}

/// Every COM object we create starts with a vtable pointer.
#[repr(C)]
pub struct ComObject<T> {
    pub vtbl: *const T,
}

impl Guid {
    pub const fn is(&self, other: &Guid) -> bool {
        self.data1 == other.data1
            && self.data2 == other.data2
            && self.data3 == other.data3
            && self.data4[0] == other.data4[0]
            && self.data4[1] == other.data4[1]
            && self.data4[2] == other.data4[2]
            && self.data4[3] == other.data4[3]
            && self.data4[4] == other.data4[4]
            && self.data4[5] == other.data4[5]
            && self.data4[6] == other.data4[6]
            && self.data4[7] == other.data4[7]
    }
}

#[link(name = "oleaut32")]
extern "system" {
    pub fn SysAllocStringLen(src: *const u16, len: u32) -> BSTR;
    pub fn SysFreeString(bstr: BSTR);
    pub fn SysStringLen(bstr: BSTR) -> u32;
}

/// Read a `BSTR` into a `String` without taking ownership.
pub unsafe fn bstr_to_string(b: BSTR) -> String {
    if b.is_null() {
        return String::new();
    }
    let len = SysStringLen(b);
    if len == 0 {
        return String::new();
    }
    let slice = core::slice::from_raw_parts(b, len as usize);
    String::from_utf16_lossy(slice)
}

#[link(name = "ole32")]
extern "system" {
    pub fn CoTaskMemFree(p: *mut c_void);
}
