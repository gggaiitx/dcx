//! Every IID this crate uses, one named constant per interface, plus the
//! generator for parameterized (`IList<…>`) interface IIDs.
//!
//! # Where the numbers come from
//!
//! Plain interface IIDs: `dcx/docs/winrt-abi.md`, which `tools/metadump` writes
//! straight from `C:\Windows\System32\WinMetadata\*.winmd`. Each constant below
//! names the interface; none of them are typed from memory.
//!
//! Parameterized IIDs (`IVector<T>` a.k.a. `IList<T>`, `IIterable<KVP>`, …) do
//! not exist in any `.winmd`: they are computed. The algorithm is the one
//! C++/WinRT implements in `winrt/base.h` (`generate_guid`) and the OS implements
//! in `RoGetParameterizedTypeInstanceIID`:
//!
//! ```text
//! IID = SHA-1( namespace_guid_bytes || ascii("pinterface(<generic iid>;" <arg1> ";" …)") )
//!       [0..16), with the version nibble forced to 5 and the variant bits to 10
//! ```
//!
//! where `namespace_guid_bytes` is the well-known name-based namespace GUID
//! `d57af411-737b-c042-abae-878b1e16adee` in its in-memory (little-endian) form,
//! `<generic iid>` is the *open generic* interface IID (e.g.
//! `913337e9-11a1-4345-a3a2-4e7f956e222d` for `IVector`\`1), and the argument
//! signatures are the WinRT type signatures: `rc(<RuntimeClass>;<default iid>)`
//! for a runtime class, `pinterface(...)` for a parameterized interface,
//! `cinterface(IInspectable)` for `Object`, `i4`/`b1`/`string`/… for primitives.
//!
//! The constants in this module were printed by a throwaway C++ program compiled
//! against `D:\SoftWare\Windhawk\Compiler\include\winrt` (the exact C++/WinRT
//! headers Windhawk compiles mods with) calling
//! `winrt::guid_of<winrt::Windows::Foundation::Collections::…>()`, and the unit
//! tests at the bottom recompute them from their signature strings with
//! [`pinterface_iid`], so a typo in either the constant or the generator fails
//! `cargo test -p dcx-xaml`.

use crate::com::Guid;

// ---------------------------------------------------------------------------
// Windows.UI.Xaml — `metadump --iface <name>` on Windows.UI.Xaml.winmd
// ---------------------------------------------------------------------------

/// `Windows.UI.Xaml.Markup.IXamlReaderStatics` (`Load`, absolute slot 6).
pub const IID_IXAMLREADERSTATICS: Guid = Guid::new(
    0x9891_C6BD,
    0x534F,
    0x4955,
    [0xB8, 0x5A, 0x8A, 0x8D, 0xC0, 0xDC, 0xA6, 0x02],
);

/// `Windows.UI.Xaml.IStyle` (`get_Setters`, absolute slot 7).
pub const IID_ISTYLE: Guid = Guid::new(
    0xC4A9_F225,
    0x9DB7,
    0x4A7D,
    [0xB6, 0xD1, 0xF7, 0x4E, 0xDB, 0x92, 0x93, 0xC2],
);

/// `Windows.UI.Xaml.ISetter` (`get_Property` slot 6, `get_Value` slot 8).
pub const IID_ISETTER: Guid = Guid::new(
    0xA73D_ED29,
    0xB4AE,
    0x4A81,
    [0xBE, 0x85, 0xE6, 0x90, 0xBA, 0x0D, 0x3B, 0x6E],
);

/// `Windows.UI.Xaml.ISetterBase` — the default interface of `SetterBase`, i.e.
/// what `IVector<SetterBase>::GetAt` returns before QI'ing `ISetter`.
pub const IID_ISETTERBASE: Guid = Guid::new(
    0x418B_E27C,
    0x2AC4,
    0x4F22,
    [0x80, 0x97, 0xDE, 0xA3, 0xAE, 0xEB, 0x2F, 0xB3],
);

/// `Windows.UI.Xaml.ISetterBaseCollection` — the default interface of the
/// `SetterBaseCollection` class, i.e. the pointer `IStyle::get_Setters` returns.
pub const IID_ISETTERBASECOLLECTION: Guid = Guid::new(
    0x03C4_0CA8,
    0x909E,
    0x4117,
    [0x81, 0x1C, 0xA4, 0x52, 0x94, 0x96, 0xBD, 0xF1],
);

/// `Windows.UI.Xaml.IDependencyObject` (`GetValue` 6, `SetValue` 7,
/// `ClearValue` 8, `ReadLocalValue` 9, `GetAnimationBaseValue` 10).
pub const IID_IDEPENDENCYOBJECT: Guid = Guid::new(
    0x5C52_6665,
    0xF60E,
    0x4912,
    [0xAF, 0x59, 0x5F, 0xE0, 0x68, 0x0F, 0x08, 0x9D],
);

/// `Windows.UI.Xaml.IDependencyObject2` (`RegisterPropertyChangedCallback` 6,
/// `UnregisterPropertyChangedCallback` 7).
pub const IID_IDEPENDENCYOBJECT2: Guid = Guid::new(
    0x29FE_D85D,
    0x3D22,
    0x43A1,
    [0xAD, 0xD0, 0x17, 0x02, 0x7C, 0x08, 0xB2, 0x12],
);

/// `Windows.UI.Xaml.DependencyPropertyChangedCallback` — the delegate ABI
/// interface. WinRT delegates derive from `IUnknown`, so `Invoke` is slot 3
/// (not slot 6 as `metadump --iface` prints for it: that listing uses the
/// IInspectable base convention, which does not apply to delegates).
pub const IID_DEPENDENCY_PROPERTY_CHANGED_CALLBACK: Guid = Guid::new(
    0x4588_3D16,
    0x27BF,
    0x4BC1,
    [0xAC, 0x26, 0x94, 0xC1, 0x60, 0x1F, 0x3A, 0x49],
);

/// `Windows.UI.Xaml.IFrameworkElement` (`get_Tag` 9, `get_ActualWidth` 13,
/// `get_Name` 33, `put_Style` 39).
pub const IID_IFRAMEWORKEL: Guid = Guid::new(
    0xA391_D09B,
    0x4A99,
    0x4B7C,
    [0x9D, 0x8D, 0x6F, 0xA5, 0xD0, 0x1F, 0x6F, 0xBF],
);

/// `Windows.UI.Xaml.IUIElement` (`put_Visibility` absolute slot 22).
pub const IID_IUIELEMENT: Guid = Guid::new(
    0x676D_0BE9,
    0xB65C,
    0x41C6,
    [0xBA, 0x40, 0x58, 0xCF, 0x87, 0xF2, 0x01, 0xC1],
);

/// `Windows.UI.Xaml.Controls.IGrid` (`get_ColumnDefinitions` slot 7).
pub const IID_IGRID: Guid = Guid::new(
    0xFD10_4460,
    0x2E15,
    0x4BA3,
    [0x8B, 0x8F, 0xFA, 0x69, 0x3A, 0x41, 0x61, 0xE9],
);

/// `Windows.UI.Xaml.IUIElement10` (`get_ActualOffset` slot 6,
/// `get_ActualSize` slot 7).
///
/// `ActualOffset` is the only way to learn where XAML put an element: it is a
/// read-only property with no `DependencyProperty`, so the `SetValue` path
/// cannot be used and `FrameworkElement.ActualWidth` alone gives no position.
pub const IID_IUIELEMENT10: Guid = Guid::new(
    0xD531_C629,
    0xAD2C,
    0x5F6B,
    [0xAD, 0xCF, 0xFB, 0x87, 0x28, 0x7D, 0x18, 0xD7],
);

/// `Windows.UI.Xaml.Media.ICompositeTransform` — one mutable object carrying
/// both scale and translation, so a hover frame is four `put_` calls on the same
/// interface instead of replacing the element's `RenderTransform`.
///
/// `put_CenterX` 7, `put_CenterY` 9, `put_ScaleX` 11, `put_ScaleY` 13,
/// `put_TranslateX` 21, `put_TranslateY` 23.
pub const IID_ICOMPOSITETRANSFORM: Guid = Guid::new(
    0xC8A4_385B,
    0xF24A,
    0x4701,
    [0xA2, 0x65, 0xA7, 0x88, 0x46, 0xF1, 0x42, 0xB9],
);

/// `Windows.UI.Xaml.Controls.IColumnDefinition` (`get_Width` 6, `put_Width` 7,
/// `get_MaxWidth` 8, `put_MaxWidth` 9, `get_MinWidth` 10, `put_MinWidth` 11).
pub const IID_ICOLUMNDEFINITION: Guid = Guid::new(
    0xF7F1_B229,
    0xF024,
    0x467F,
    [0x97, 0x0A, 0x7E, 0x70, 0x56, 0x15, 0xDB, 0x7B],
);

/// `Windows.UI.Xaml.Controls.IGridFactory` (`CreateInstance` slot 6) —
/// `metadump --iface Windows.UI.Xaml.Controls.IGridFactory`.
///
/// `Grid` is a *composable* class: C++/WinRT's `Controls::Grid{}` (which the
/// reference uses at 15126) goes through this factory, not through
/// `IActivationFactory`.
pub const IID_IGRIDFACTORY: Guid = Guid::new(
    0xAE81_4041,
    0xC531,
    0x43B4,
    [0xBF, 0x99, 0x12, 0xF5, 0x06, 0xF7, 0xB0, 0x1C],
);

/// `Windows.UI.Xaml.IResourceDictionary` — the default interface of the
/// `ResourceDictionary` class that `GetStyleFromXamlSetters` loads.
pub const IID_IRESOURCEDICTIONARY: Guid = Guid::new(
    0xC1EA_4F24,
    0xD6DE,
    0x4191,
    [0x8E, 0x3A, 0xF4, 0x86, 0x01, 0xF7, 0x48, 0x9C],
);

/// `Windows.UI.Xaml.IVisualStateManagerStatics` (`GetVisualStateGroups` slot 6).
pub const IID_IVISUALSTATEMANAGERSTATICS: Guid = Guid::new(
    0x01D0_E9E0,
    0xD713,
    0x414E,
    [0xA7, 0x4E, 0xE6, 0x3E, 0xC7, 0xAC, 0x8C, 0x3D],
);

/// `Windows.UI.Xaml.IVisualStateGroup` (`get_Name` 6, `get_States` 8,
/// `get_CurrentState` 9).
pub const IID_IVISUALSTATEGROUP: Guid = Guid::new(
    0xE4F9_D9A4,
    0xE028,
    0x44DE,
    [0x9B, 0x15, 0x49, 0x29, 0xAE, 0x0A, 0x26, 0xC2],
);

/// `Windows.UI.Xaml.IVisualState` (`get_Name` slot 6).
pub const IID_IVISUALSTATE: Guid = Guid::new(
    0x6320_AFFC,
    0xC31A,
    0x4450,
    [0xAF, 0xDE, 0xF6, 0xEA, 0x7B, 0xD1, 0xF5, 0x86],
);

/// `Windows.UI.Xaml.Media.IVisualTreeHelperStatics` (`GetChild` 10,
/// `GetChildrenCount` 11, `GetParent` 12).
pub const IID_IVISUALTREEHELPERSTATICS: Guid = Guid::new(
    0xE757_58C4,
    0xD25D,
    0x4B1D,
    [0x97, 0x1F, 0x59, 0x6F, 0x17, 0xF1, 0x2B, 0xAA],
);

/// `Windows.UI.Xaml.IDependencyPropertyStatics` (`get_UnsetValue` slot 6) —
/// `metadump --iface Windows.UI.Xaml.IDependencyPropertyStatics`. Needed to
/// recognise `DependencyProperty.UnsetValue`, which `ReadLocalValue` returns for
/// "no local value" (see `ReadLocalValueWithWorkaround`, reference 12441).
pub const IID_IDEPENDENCYPROPERTYSTATICS: Guid = Guid::new(
    0x49E5_F28F,
    0x8259,
    0x4D5C,
    [0xAA, 0xE0, 0x83, 0xD5, 0x6D, 0xBB, 0x68, 0xD9],
);

/// `Windows.Foundation.IPropertyValue` — boxed primitives, unboxed by
/// `TryUnboxPropertyValue` (reference 15848) for `[Prop=Value]` matchers.
pub const IID_IPROPERTYVALUE: Guid = Guid::new(
    0x4BD6_82DD,
    0x7554,
    0x40E9,
    [0x9A, 0x9B, 0x82, 0x65, 0x4E, 0xDE, 0x7E, 0x62],
);

/// `Windows.Foundation.IActivationFactory` — resolved from C++/WinRT `base.h`
/// line 1994 (`00000035-0000-0000-c000-000000000046`).
pub const IID_IACTIVATIONFACTORY: Guid = Guid::new(
    0x0000_0035,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);

// ---------------------------------------------------------------------------
// Parameterized interfaces (computed IIDs, see module docs)
// ---------------------------------------------------------------------------

/// `Windows.Foundation.Collections.IVector`1<Windows.UI.Xaml.SetterBase>`, i.e.
/// the ABI `IList<SetterBase>`.
///
/// Signature (C++/WinRT `signature<…>`):
/// `pinterface({913337e9-11a1-4345-a3a2-4e7f956e222d};rc(Windows.UI.Xaml.SetterBase;{418be27c-2ac4-4f22-8097-dea3aeeb2fb3}))`
pub const IID_IVECTOR_SETTERBASE: Guid = Guid::new(
    0xBD55_A7FD,
    0x7501,
    0x5519,
    [0x9F, 0xFB, 0xD0, 0x07, 0x42, 0x40, 0xDB, 0x61],
);

/// `Windows.Foundation.Collections.IIterable`1<Windows.Foundation.Collections.IKeyValuePair`2<Object,Object>>`
/// — `ResourceDictionary.First()`, the way `GetStyleFromXamlSetters` reads the
/// `Style` back out of the loaded dictionary.
///
/// Signature:
/// `pinterface({faa585ea-6214-4217-afda-7f46de5869b3};pinterface({02b51929-c1c4-4a7e-8940-0312b5c18500};cinterface(IInspectable);cinterface(IInspectable)))`
pub const IID_IITERABLE_KVP_OBJECT_OBJECT: Guid = Guid::new(
    0x33AC_68F0,
    0x1084,
    0x529A,
    [0x8A, 0x17, 0x4E, 0x7C, 0x8A, 0xDB, 0x7A, 0x0C],
);

/// `Windows.Foundation.Collections.IMap`2<Object,Object>` — `ResourceDictionary`
/// as an `IDictionary` (needed only if the engine ever inserts resources).
///
/// Signature: `pinterface({3c2925fe-8519-45c1-aa79-197b6718c1c1};cinterface(IInspectable);cinterface(IInspectable))`
pub const IID_IMAP_OBJECT_OBJECT: Guid = Guid::new(
    0xF5F6_9427,
    0x55ED,
    0x5512,
    [0x84, 0x29, 0xD4, 0xF6, 0x62, 0x6D, 0xFC, 0xDD],
);

/// `Windows.Foundation.Collections.IVector`1<Windows.UI.Xaml.Controls.ColumnDefinition>`
/// — the default interface of `ColumnDefinitionCollection`, i.e. what
/// `IGrid::get_ColumnDefinitions` returns.
///
/// Signature: `pinterface({913337e9-11a1-4345-a3a2-4e7f956e222d};rc(Windows.UI.Xaml.Controls.ColumnDefinition;{f7f1b229-f024-467f-970a-7e705615db7b}))`
pub const IID_IVECTOR_COLUMNDEFINITION: Guid = Guid::new(
    0x6E13_7874,
    0xDD39,
    0x5B4F,
    [0xA7, 0xEC, 0x2E, 0xA6, 0x9E, 0x4C, 0x22, 0x29],
);

/// `Windows.Foundation.Collections.IVector`1<Windows.UI.Xaml.VisualStateGroup>`
/// — used to cross-check what `GetVisualStateGroups` hands back.
///
/// Signature: `pinterface({913337e9-11a1-4345-a3a2-4e7f956e222d};rc(Windows.UI.Xaml.VisualStateGroup;{e4f9d9a4-e028-44de-9b15-4929ae0a26c2}))`
pub const IID_IVECTOR_VISUALSTATEGROUP: Guid = Guid::new(
    0x729A_0392,
    0x675E,
    0x58E4,
    [0xB6, 0xA3, 0x04, 0x57, 0xBB, 0x9E, 0xA9, 0x68],
);

/// `Windows.Foundation.IReference`1<Int32>` — the boxed-int form XAML uses for
/// enum-typed dependency properties and for `OtherType` property values
/// (reference 15885 does `value.try_as<int32_t>()`).
///
/// Signature: `pinterface({61c17706-2d65-11e0-9ae8-d48564015472};i4)`
pub const IID_IREFERENCE_INT32: Guid = Guid::new(
    0x548C_EFBD,
    0xBC8A,
    0x5FA0,
    [0x8D, 0xF2, 0x95, 0x74, 0x40, 0xFC, 0x8B, 0xF4],
);

// ---------------------------------------------------------------------------
// Parameterized IID generation (verified against C++/WinRT, see tests)
// ---------------------------------------------------------------------------

/// The name-based IID namespace GUID `d57af411-737b-c042-abae-878b1e16adee`, in
/// its in-memory byte order (`to_array(guid const&)` in winrt/base.h, line 1077).
const IID_NAMESPACE_BYTES: [u8; 16] = [
    0x11, 0xF4, 0x7A, 0xD5, // d57af411, little-endian
    0x7B, 0x73, // 737b, little-endian
    0x42, 0xC0, // c042, little-endian
    0xAB, 0xAE, 0x87, 0x8B, 0x1E, 0x16, 0xAD, 0xEE,
];

/// The `{guid}` text form C++/WinRT embeds in a type signature
/// (`to_array<char>(guid const&)`, winrt/base.h line 1109): lowercase, braced.
pub fn guid_signature_text(g: &Guid) -> String {
    g.to_braced()
}

/// Signature of a runtime class used as a type argument:
/// `rc(<Namespace.Type>;<default interface signature>)`.
///
/// A runtime class argument is identified by its name plus its *default
/// interface* IID (winrt/base.h line 1547).
pub fn runtime_class_signature(class_name: &str, default_interface: &Guid) -> String {
    format!(
        "rc({};{})",
        class_name,
        guid_signature_text(default_interface)
    )
}

/// Signature of a parameterized interface used as a type argument (or as the
/// outermost type): `pinterface(<open generic IID>;<arg>;<arg>…)`.
pub fn generic_signature(generic_iid: &Guid, args: &[&str]) -> String {
    let mut s = format!("pinterface({}", guid_signature_text(generic_iid));
    for a in args {
        s.push(';');
        s.push_str(a);
    }
    s.push(')');
    s
}

/// Signature of an `Object` (`Windows.Foundation.IInspectable`) type argument,
/// winrt/base.h line 1485.
pub const SIGNATURE_OBJECT: &str = "cinterface(IInspectable)";
/// Signature of `Int32`, winrt/base.h line 1474.
pub const SIGNATURE_I4: &str = "i4";
/// Signature of `Boolean`, winrt/base.h line 1471.
pub const SIGNATURE_B1: &str = "b1";

/// Open generic IIDs of the collection interfaces (the `IVector`\`1 etc. blocks
/// of docs/winrt-abi.md).
pub const IID_GENERIC_IVECTOR1: Guid = Guid::new(
    0x9133_37E9,
    0x11A1,
    0x4345,
    [0xA3, 0xA2, 0x4E, 0x7F, 0x95, 0x6E, 0x22, 0x2D],
);
pub const IID_GENERIC_IITERABLE1: Guid = Guid::new(
    0xFAA5_85EA,
    0x6214,
    0x4217,
    [0xAF, 0xDA, 0x7F, 0x46, 0xDE, 0x58, 0x69, 0xB3],
);
pub const IID_GENERIC_IMAP2: Guid = Guid::new(
    0x3C29_25FE,
    0x8519,
    0x45C1,
    [0xAA, 0x79, 0x19, 0x7B, 0x67, 0x18, 0xC1, 0xC1],
);
pub const IID_GENERIC_IKEYVALUEPAIR2: Guid = Guid::new(
    0x02B5_1929,
    0xC1C4,
    0x4A7E,
    [0x89, 0x40, 0x03, 0x12, 0xB5, 0xC1, 0x85, 0x00],
);
pub const IID_GENERIC_IREFERENCE1: Guid = Guid::new(
    0x61C1_7706,
    0x2D65,
    0x11E0,
    [0x9A, 0xE8, 0xD4, 0x85, 0x64, 0x01, 0x54, 0x72],
);

/// Compute a parameterized interface IID from its WinRT type signature.
///
/// Mirrors `winrt::impl::generate_guid` (winrt/base.h line 1354): SHA-1 over the
/// namespace GUID bytes followed by the ASCII signature, first 16 bytes, with
/// the RFC-4122 version-5 / variant-10 bits forced.
///
/// Byte order, spelled out because it is the subtle part of the C++ code:
/// `to_guid(hash)` reads the digest big-endian into `Data1/Data2/Data3` and then
/// `endian_swap` byte-swaps those three fields — i.e. `Data1 = BE32(h[0..4])`,
/// `Data2 = BE16(h[4..6])`, `Data3 = BE16(h[6..8]) & 0x0fff | 0x5000`, and
/// `Data4 = h[8..16]` with the variant bits on `Data4[0]`.
pub fn pinterface_iid(signature: &str) -> Guid {
    let mut input = Vec::with_capacity(16 + signature.len());
    input.extend_from_slice(&IID_NAMESPACE_BYTES);
    input.extend_from_slice(signature.as_bytes());

    let h = sha1(&input);
    Guid::new(
        u32::from_be_bytes([h[0], h[1], h[2], h[3]]),
        u16::from_be_bytes([h[4], h[5]]),
        (((h[6] as u16) & 0x0F) << 8) | (h[7] as u16) | 0x5000,
        [
            (h[8] & 0x3F) | 0x80,
            h[9],
            h[10],
            h[11],
            h[12],
            h[13],
            h[14],
            h[15],
        ],
    )
}

/// SHA-1 (FIPS 180-1). `std`-only; only used to derive IIDs, never in a hot
/// path.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    // Message length in bits, appended big-endian after the 0x80 padding.
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut tail = Vec::with_capacity(64);
    tail.push(0x80u8);
    while (data.len() + tail.len()) % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&bit_len.to_be_bytes());

    let mut msg = Vec::with_capacity(data.len() + tail.len());
    msg.extend_from_slice(data);
    msg.extend_from_slice(&tail);

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(wi.wrapping_add(k));
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for i in 0..5 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known_vectors() {
        assert_eq!(
            sha1(b""),
            [
                0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60,
                0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09
            ]
        );
        assert_eq!(
            sha1(b"abc"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d
            ]
        );
        // > 64 bytes: exercises multiple blocks and the length padding.
        assert_eq!(
            sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            [
                0x84, 0x98, 0x3e, 0x44, 0x1c, 0x3b, 0xd2, 0x6e, 0xba, 0xae, 0x4a, 0xa1, 0xf9, 0x51,
                0x29, 0xe5, 0xe5, 0x46, 0x70, 0xf1
            ]
        );
    }

    // --- parameterized IIDs, against the C++/WinRT values -------------------

    #[test]
    fn pinterface_setterbase() {
        let arg = runtime_class_signature("Windows.UI.Xaml.SetterBase", &IID_ISETTERBASE);
        let sig = generic_signature(&IID_GENERIC_IVECTOR1, &[&arg]);
        assert_eq!(
            pinterface_iid(&sig).to_braced(),
            "{bd55a7fd-7501-5519-9ffb-d0074240db61}"
        );
        assert!(pinterface_iid(&sig).is(&IID_IVECTOR_SETTERBASE));
    }

    #[test]
    fn pinterface_iterable_kvp_object_object() {
        let kvp = generic_signature(
            &IID_GENERIC_IKEYVALUEPAIR2,
            &[SIGNATURE_OBJECT, SIGNATURE_OBJECT],
        );
        let sig = generic_signature(&IID_GENERIC_IITERABLE1, &[&kvp]);
        assert_eq!(
            pinterface_iid(&sig).to_braced(),
            "{33ac68f0-1084-529a-8a17-4e7c8adb7a0c}"
        );
        assert!(pinterface_iid(&sig).is(&IID_IITERABLE_KVP_OBJECT_OBJECT));
    }

    #[test]
    fn pinterface_map_object_object() {
        let sig = generic_signature(&IID_GENERIC_IMAP2, &[SIGNATURE_OBJECT, SIGNATURE_OBJECT]);
        assert_eq!(
            pinterface_iid(&sig).to_braced(),
            "{f5f69427-55ed-5512-8429-d4f6626dfcdd}"
        );
        assert!(pinterface_iid(&sig).is(&IID_IMAP_OBJECT_OBJECT));
    }

    #[test]
    fn pinterface_column_definition_collection() {
        let arg = runtime_class_signature(
            "Windows.UI.Xaml.Controls.ColumnDefinition",
            &IID_ICOLUMNDEFINITION,
        );
        let sig = generic_signature(&IID_GENERIC_IVECTOR1, &[&arg]);
        assert_eq!(
            pinterface_iid(&sig).to_braced(),
            "{6e137874-dd39-5b4f-a7ec-2ea69e4c2229}"
        );
        assert!(pinterface_iid(&sig).is(&IID_IVECTOR_COLUMNDEFINITION));
    }

    #[test]
    fn pinterface_visual_state_group() {
        let arg =
            runtime_class_signature("Windows.UI.Xaml.VisualStateGroup", &IID_IVISUALSTATEGROUP);
        let sig = generic_signature(&IID_GENERIC_IVECTOR1, &[&arg]);
        assert_eq!(
            pinterface_iid(&sig).to_braced(),
            "{729a0392-675e-58e4-b6a3-0457bb9ea968}"
        );
        assert!(pinterface_iid(&sig).is(&IID_IVECTOR_VISUALSTATEGROUP));
    }

    #[test]
    fn pinterface_reference_int32() {
        let sig = generic_signature(&IID_GENERIC_IREFERENCE1, &[SIGNATURE_I4]);
        assert_eq!(
            pinterface_iid(&sig).to_braced(),
            "{548cefbd-bc8a-5fa0-8df2-957440fc8bf4}"
        );
        assert!(pinterface_iid(&sig).is(&IID_IREFERENCE_INT32));
    }

    /// Primitive type arguments are a different branch of C++/WinRT's
    /// `signature<…>` than runtime classes.
    #[test]
    fn pinterface_primitives() {
        let iv4 = generic_signature(&IID_GENERIC_IVECTOR1, &[SIGNATURE_I4]);
        assert_eq!(
            pinterface_iid(&iv4).to_braced(),
            "{b939af5b-b45d-5489-9149-61442c1905fe}"
        );
        let ii4 = generic_signature(&IID_GENERIC_IITERABLE1, &[SIGNATURE_I4]);
        assert_eq!(
            pinterface_iid(&ii4).to_braced(),
            "{81a643fb-f51c-5565-83c4-f96425777b66}"
        );
        let ib1 = generic_signature(&IID_GENERIC_IREFERENCE1, &[SIGNATURE_B1]);
        assert_eq!(
            pinterface_iid(&ib1).to_braced(),
            "{3c00fd60-2950-5939-a21a-2d12c5a01b8a}"
        );
    }

    /// `IVector<Object>` — `Object` as an argument is `cinterface(IInspectable)`.
    #[test]
    fn pinterface_vector_object() {
        let sig = generic_signature(&IID_GENERIC_IVECTOR1, &[SIGNATURE_OBJECT]);
        assert_eq!(
            pinterface_iid(&sig).to_braced(),
            "{b32bdca4-5e52-5b27-bc5d-d66a1a268c2a}"
        );
    }
}
