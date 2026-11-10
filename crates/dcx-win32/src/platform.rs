//! 操作系统版本识别与「任务栏样式」能力判定。
//!
//! 为什么宿主需要它：`dcx_engine.dll` 只认 Windows 11 的 XAML 任务栏。它在
//! Explorer 里用 `FindWindowExW(Shell_TrayWnd, …, DesktopWindowContentBridge)`
//! 找 XAML 宿主；Windows 10 及更早版本的任务栏没有这个子窗口，注入进去的
//! 引擎会一遍遍重试、永远连不上，同时还白占一个 DLL、一个钩子和轮询线程。
//! 因此宿主在启动期就判定一次：不支持则**不注入、不轮询**，设置中心整页停用。
//!
//! 版本来源：`RtlGetVersion`（`GetVersionExW` 在没有兼容性清单的进程里会把
//! Win10/11 一律报成 6.2，不能用）。显示版本（23H2）、补丁号（UBR）、版本
//! 类型来自 `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`；注册表读取
//! 失败只影响展示，不影响 `RtlGetVersion` 给出的能力判定。

use std::sync::OnceLock;

/// Windows 11 首个公开版本（21H2）的内部版本号。任务栏从这里起换成 XAML 实现。
pub const WIN11_BUILD: u32 = 22000;

const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const KEY_QUERY_VALUE: u32 = 0x0001;
/// 32 位进程读 64 位系统的注册表视图，否则会被 WOW64 重定向。
const KEY_WOW64_64KEY: u32 = 0x0100;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;
const ERROR_SUCCESS: i32 = 0;
const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

#[repr(C)]
struct OSVERSIONINFOW {
    dw_os_version_info_size: u32,
    dw_major_version: u32,
    dw_minor_version: u32,
    dw_build_number: u32,
    dw_platform_id: u32,
    sz_csd_version: [u16; 128],
}

type RtlGetVersionFn = unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsInfo {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
    /// 例如 `23H2`；部分精简镜像里没有这个值。
    pub display_version: String,
    /// Update Build Revision，即 `22631.3155` 的小数部分。
    pub ubr: u32,
    /// `Professional` / `Home` / `Enterprise` …
    pub edition_id: String,
    /// 注册表里的 `ProductName`。Win11 会误报成 "Windows 10 Pro"，所以只用于
    /// 识别 Server 和作为兜底显示，不参与 Win10/Win11 的判定。
    pub product_name: String,
    /// `InstallationType == "Server"`。
    pub is_server: bool,
    /// 版本号来源，便于「无法识别」时给出可诊断的提示。
    pub source: &'static str,
}

static INFO: OnceLock<OsInfo> = OnceLock::new();

/// 进程内只探测一次：注册表读取和动态符号解析都不便宜，而且每一帧都要问。
pub fn os_info() -> &'static OsInfo {
    INFO.get_or_init(detect)
}

/// 纯函数，便于单测：只有 NT 10.0 build ≥ 22000（或更新的 NT 主版本）才有
/// XAML 任务栏。Server 2022 是 20348 → 停用；Server 2025 是 26100 → 可用，
/// 所以按 build 判定比按 "名字里有没有 Server" 更准。
pub fn is_xaml_taskbar(major: u32, build: u32) -> bool {
    major > 10 || (major == 10 && build >= WIN11_BUILD)
}

/// 任务栏样式能力。**识别失败时一律停用**（fail-closed）：宁可少一个样式页，
/// 也不要把会无限重试的 DLL 注入 Explorer。
///
/// `DCX_FORCE_NO_STYLE=1` 只用于诊断：在 Win11 机器上模拟降级路径，检查设置
/// 中心的警示卡与灰掉状态，以及独立应用是否照常可用。默认完全不生效。
pub fn taskbar_style_supported() -> bool {
    if forced_off() {
        return false;
    }
    let i = os_info();
    i.build > 0 && is_xaml_taskbar(i.major, i.build)
}

fn flag_disables(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
}

fn forced_off() -> bool {
    std::env::var("DCX_FORCE_NO_STYLE")
        .map(|v| flag_disables(&v))
        .unwrap_or(false)
}

/// Whether the degraded state is a deliberate simulation rather than a real
/// unsupported OS. Keeps the warning banner's wording honest.
pub fn simulated() -> bool {
    forced_off()
}

/// 版本探测本身失败（RtlGetVersion 与注册表都拿不到 build）。
pub fn detection_failed() -> bool {
    os_info().build == 0
}

/// 停用的具体原因，写进设置中心警示卡与宿主日志。
pub fn reason() -> String {
    if forced_off() {
        return "由 DCX_FORCE_NO_STYLE 模拟非 Win11 环境（诊断用）".to_string();
    }
    let i = os_info();
    if i.build == 0 {
        return "无法读取系统内部版本（RtlGetVersion 与注册表都失败）".to_string();
    }
    if is_xaml_taskbar(i.major, i.build) {
        return String::new();
    }
    format!(
        "当前内部版本 {}，低于 Windows 11 的 {}；任务栏不是 XAML 实现，没有 \
         DesktopWindowContentBridge 子窗口",
        i.build, WIN11_BUILD
    )
}

/// 面向用户的系统名。Win11 的 `ProductName` 仍是 "Windows 10 Pro"，
/// 所以名称由 build 推，版本类型由 `EditionID` 补。
pub fn display_name() -> String {
    let i = os_info();
    if i.build == 0 {
        return "未知 Windows 版本".to_string();
    }
    if i.is_server {
        if !i.product_name.is_empty() {
            return i.product_name.clone();
        }
        return format!("Windows Server（内部版本 {}）", i.build);
    }
    let base = if i.major > 10 {
        "Windows"
    } else if i.major == 10 && i.build >= WIN11_BUILD {
        "Windows 11"
    } else if i.major == 10 {
        "Windows 10"
    } else if i.major == 6 && i.minor == 3 {
        "Windows 8.1"
    } else if i.major == 6 && i.minor == 2 {
        "Windows 8"
    } else if i.major == 6 && i.minor == 1 {
        "Windows 7"
    } else {
        "Windows"
    };
    let edition = edition_label(&i.edition_id);
    if edition.is_empty() {
        base.to_string()
    } else {
        format!("{base} {edition}")
    }
}

/// 版本细节：`23H2 · 内部版本 22631.3155`。
pub fn version_detail() -> String {
    let i = os_info();
    if i.build == 0 {
        return "版本号不可用".to_string();
    }
    let mut parts = Vec::new();
    if !i.display_version.is_empty() {
        parts.push(i.display_version.clone());
    }
    let mut build = format!("内部版本 {}", i.build);
    if i.ubr > 0 {
        build.push_str(&format!(".{}", i.ubr));
    }
    parts.push(build);
    parts.join(" · ")
}

/// 一行完整描述，日志与设置中心共用。
pub fn describe() -> String {
    format!("{} · {}", display_name(), version_detail())
}

fn edition_label(id: &str) -> &'static str {
    match id {
        "Professional" => "专业版",
        "ProfessionalN" => "专业版 N",
        "Home" | "Core" => "家庭版",
        "HomeN" | "CoreN" => "家庭版 N",
        "HomeSingleLanguage" => "家庭单语言版",
        "Enterprise" => "企业版",
        "EnterpriseN" => "企业版 N",
        "Education" => "教育版",
        "ProfessionalEducation" => "专业教育版",
        "ProfessionalWorkstation" => "专业工作站版",
        _ => "",
    }
}

fn detect() -> OsInfo {
    let mut v: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    v.dw_os_version_info_size = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    let rtl_ok = unsafe { rtl_get_version(&mut v) };
    let (major, minor, mut build, mut source) = if rtl_ok && v.dw_major_version != 0 {
        (
            v.dw_major_version,
            v.dw_minor_version,
            v.dw_build_number,
            "RtlGetVersion",
        )
    } else {
        (0, 0, 0, "")
    };
    if build == 0 {
        if let Some(b) = reg_string("CurrentBuild").and_then(|s| s.parse::<u32>().ok()) {
            build = b;
            source = "注册表 CurrentBuild";
        }
    }
    let display_version = reg_string("DisplayVersion").unwrap_or_default();
    let ubr = reg_dword("UBR").unwrap_or(0);
    let edition_id = reg_string("EditionID").unwrap_or_default();
    let product_name = reg_string("ProductName").unwrap_or_default();
    let is_server = reg_string("InstallationType")
        .map(|s| s.eq_ignore_ascii_case("Server"))
        .unwrap_or(false)
        || product_name.contains("Server");

    OsInfo {
        major: if major != 0 {
            major
        } else if build >= WIN11_BUILD || build >= 10240 {
            10
        } else {
            6
        },
        minor,
        build,
        display_version,
        ubr,
        edition_id,
        product_name,
        is_server,
        source: if source.is_empty() {
            "不可用"
        } else {
            source
        },
    }
}

/// `RtlGetVersion` 由 ntdll 导出但没有 SDK 导入库，动态取符号。
unsafe fn rtl_get_version(out: *mut OSVERSIONINFOW) -> bool {
    let module = crate::GetModuleHandleW(crate::wide("ntdll.dll").as_ptr());
    if module == 0 {
        return false;
    }
    let proc = crate::GetProcAddress(module, b"RtlGetVersion\0".as_ptr());
    if proc == 0 {
        return false;
    }
    let f: RtlGetVersionFn = std::mem::transmute(proc);
    f(out) == 0
}

/// 读 HKLM 的 REG_SZ。任何失败都返回 `None`：这些值只用于展示。
fn reg_string(name: &str) -> Option<String> {
    reg_query(name, |ty, data| {
        if ty != REG_SZ || data.len() < 2 {
            return None;
        }
        let mut units: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        while units.last() == Some(&0) {
            units.pop();
        }
        Some(String::from_utf16_lossy(&units))
    })
}

fn reg_dword(name: &str) -> Option<u32> {
    reg_query(name, |ty, data| {
        if ty != REG_DWORD || data.len() < 4 {
            return None;
        }
        Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    })
}

fn reg_query<T>(name: &str, parse: impl FnOnce(u32, &[u8]) -> Option<T>) -> Option<T> {
    unsafe {
        let sub = crate::wide(CURRENT_VERSION);
        let mut key: isize = 0;
        let access = KEY_QUERY_VALUE | KEY_WOW64_64KEY;
        if crate::RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, access, &mut key)
            != ERROR_SUCCESS
        {
            return None;
        }
        let value = crate::wide(name);
        let mut ty: u32 = 0;
        let mut size: u32 = 0;
        let sized = crate::RegQueryValueExW(
            key,
            value.as_ptr(),
            core::ptr::null_mut(),
            &mut ty,
            core::ptr::null_mut(),
            &mut size,
        ) == ERROR_SUCCESS;
        let mut out = None;
        if sized && size > 0 && size <= 64 * 1024 {
            let mut buf = vec![0u8; size as usize];
            let read = crate::RegQueryValueExW(
                key,
                value.as_ptr(),
                core::ptr::null_mut(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut size,
            ) == ERROR_SUCCESS;
            if read {
                buf.truncate(size as usize);
                out = parse(ty, &buf);
            }
        }
        crate::RegCloseKey(key);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xaml_taskbar_threshold() {
        assert!(!is_xaml_taskbar(10, 10240), "Win10 1507");
        assert!(!is_xaml_taskbar(10, 19045), "Win10 22H2");
        assert!(
            !is_xaml_taskbar(10, 20348),
            "Server 2022 uses the Win10 shell"
        );
        assert!(is_xaml_taskbar(10, 22000), "Win11 21H2");
        assert!(is_xaml_taskbar(10, 22631), "Win11 23H2");
        assert!(is_xaml_taskbar(10, 26100), "Server 2025 / 24H2");
        assert!(is_xaml_taskbar(11, 1), "future NT major");
        assert!(!is_xaml_taskbar(6, 7601), "Win7");
    }

    #[test]
    fn detection_is_usable_on_this_machine() {
        let i = os_info();
        assert!(i.build > 0, "版本探测必须在本机成功：{i:?}");
        assert_eq!(i.source, "RtlGetVersion");
        assert!(!display_name().is_empty());
        assert!(version_detail().contains("内部版本"));
        // Sanity check for the fail-closed path: a zero build never enables it.
        assert!(!(i.build == 0 && taskbar_style_supported()));
    }

    #[test]
    fn edition_labels_cover_the_common_skus() {
        assert_eq!(edition_label("Professional"), "专业版");
        assert_eq!(edition_label("Core"), "家庭版");
        assert_eq!(edition_label("Enterprise"), "企业版");
        assert_eq!(edition_label("SomethingElse"), "");
    }

    #[test]
    fn force_flag_is_opt_in_and_accepts_only_truthy_values() {
        assert!(flag_disables("1"));
        assert!(flag_disables("true"));
        assert!(!flag_disables(""));
        assert!(!flag_disables("0"));
        assert!(!flag_disables("false"));
        assert!(!flag_disables("FALSE"));
    }

    #[test]
    #[ignore = "diagnostic: prints what this machine reports"]
    fn dump_platform() {
        println!("{:#?}", os_info());
        println!("display  = {}", display_name());
        println!("detail   = {}", version_detail());
        println!("describe = {}", describe());
        println!("supported= {}", taskbar_style_supported());
        println!("reason   = {}", reason());
    }
}
