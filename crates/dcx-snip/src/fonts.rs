//! 已安装字族的运行时列表：GDI 枚举 + 常用项置顶。
//!
//! 字体框只能列"本机真的有"的字族，否则用户选了却看到回退字体。做法与截图
//! 工具通常的实现一致（`EnumFontFamiliesExW` 全量枚举），但有三个必须处理的
//! 细节：
//!
//! 1. **同一个字族有中英两个名字**。中文系统里 `lfFaceName` 回的是"宋体"，
//!    而 `snip.toml` 里存的是 `SimSun`（默认值也是英文名）。所以常用项要先
//!    "规范化"：用该名字再枚举一次，拿到系统真正的字族名，再以它为 `id`。
//! 2. **竖排字族**（`@宋体`）和符号字体（Wingdings）对标注没有意义，过滤掉。
//! 3. 枚举顺序随系统而变，除常用项外按名称排序，用户才能靠位置记住。

use crate::ffi::*;
use dcx_win32::font_catalog;
use dcx_win32::HDC;
use std::collections::HashMap;
use std::sync::OnceLock;

/// 一个字族：`id` 是 GDI 字族名（写进 TOML 的就是它），`label` 是显示名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Face {
    pub id: String,
    pub label: String,
}

static FACES: OnceLock<Vec<Face>> = OnceLock::new();
/// 小写名字 → 系统真实字族名（含"SimSun → 宋体"这类中英对应）。
static CANON: OnceLock<HashMap<String, String>> = OnceLock::new();

/// 全部可选字族：常用项在前，其余按名称排序。
pub fn all() -> &'static [Face] {
    FACES.get_or_init(build)
}

/// 系统真实字族名；未收录时返回 `None`。
pub fn canonical(id: &str) -> Option<&'static str> {
    // 先建表：`CANON` 是在 `all()` 的初始化里填的，谁先被调用都不能拿到空表。
    let _ = all();
    CANON
        .get()?
        .get(&id.trim().to_lowercase())
        .map(|s| s.as_str())
}

/// 目录下标，供字体浮层标出当前项。入参可以是英文名、中文名或旧配置里的值。
pub fn index(id: &str) -> Option<usize> {
    let wanted = match canonical(id) {
        Some(v) => v,
        None => id.trim(),
    };
    all().iter().position(|f| f.id.eq_ignore_ascii_case(wanted))
}

/// 显示名：命中列表用列表里的名字，否则原样返回（旧配置里的别名）。
pub fn label_of<'a>(id: &'a str) -> &'a str {
    match index(id) {
        Some(i) => all()[i].label.as_str(),
        None => id.trim(),
    }
}

fn build() -> Vec<Face> {
    // (Face, 常用项顺序, 是否中文字族)。排序键决定列表顺序：中文字族在前，
    // 常用项在中文组内保持目录顺序，其余按名称排。
    let mut out: Vec<(Face, Option<usize>, bool)> = Vec::new();
    let mut canon: HashMap<String, String> = HashMap::new();
    let mut seen: Vec<String> = Vec::new();

    unsafe {
        let dc = GetDC(0);
        if dc != 0 {
            // 常用项先落地：它们的 `id` 统一成系统真实字族名。
            for (order, f) in font_catalog::FACES.iter().enumerate() {
                if let Some(real) = resolve_family(dc, f.id) {
                    canon.insert(f.id.to_lowercase(), real.clone());
                    canon.insert(real.to_lowercase(), real.clone());
                    let key = real.to_lowercase();
                    if !seen.contains(&key) {
                        seen.push(key);
                        let cjk = is_cjk_name(&real);
                        // 系统已经给出中文名就用它。否则"Microsoft YaHei UI"
                        // 会被贴上"微软雅黑"的标签，和枚举出来的另一个字族
                        // "微软雅黑"在列表里撞成两行同名。
                        let label = if cjk {
                            real.clone()
                        } else {
                            f.label.to_string()
                        };
                        out.push((Face { id: real, label }, Some(order), cjk));
                    }
                }
            }

            let mut rest: Vec<(String, bool)> = Vec::new();
            let mut lf = LOGFONTW::default();
            lf.lfCharSet = DEFAULT_CHARSET as u8;
            let mut ctx = Collector { out: &mut rest };
            EnumFontFamiliesExW(
                dc,
                &lf,
                collect_proc,
                &mut ctx as *mut Collector<'_> as isize,
                0,
            );
            ReleaseDC(0, dc);

            for (name, cjk) in rest {
                let key = name.to_lowercase();
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                out.push((
                    Face {
                        label: font_catalog::label(&name).to_string(),
                        id: name,
                    },
                    None,
                    cjk,
                ));
            }
        }
    }

    if out.is_empty() {
        // 枚举失败时至少给出常用项，字体框不会空着。
        for (order, f) in font_catalog::FACES.iter().enumerate() {
            out.push((
                Face {
                    id: f.id.to_string(),
                    label: f.label.to_string(),
                },
                Some(order),
                false,
            ));
        }
    }

    // 中文字族在前；常用项排在中文组最前（保持目录顺序）；组内按名称。
    out.sort_by(|a, b| {
        let rank = |x: &(Face, Option<usize>, bool)| match (x.2, x.1.is_some()) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.1.unwrap_or(usize::MAX).cmp(&b.1.unwrap_or(usize::MAX)))
            .then_with(|| a.0.label.to_lowercase().cmp(&b.0.label.to_lowercase()))
    });

    let _ = CANON.set(canon);
    out.into_iter().map(|(f, _, _)| f).collect()
}

/// 中文名（含 CJK 汉字）判定。枚举时还会看字符集，见 [`collect_proc`]。
fn is_cjk_name(name: &str) -> bool {
    name.chars().any(|c| {
        let u = c as u32;
        (0x3400..=0x9FFF).contains(&u) || (0xF900..=0xFAFF).contains(&u)
    })
}

struct Collector<'a> {
    /// (字族名, 是否中文字族)。同一字族可能按多个字符集各枚举一次，用 OR 合并。
    out: &'a mut Vec<(String, bool)>,
}

unsafe extern "system" fn collect_proc(
    lf: *const ENUMLOGFONTEXW,
    _metrics: *const core::ffi::c_void,
    _kind: u32,
    lparam: isize,
) -> i32 {
    if lf.is_null() || lparam == 0 {
        return 0;
    }
    let logfont = unsafe { &(*lf).elfLogFont };
    if logfont.lfCharSet != SYMBOL_CHARSET {
        let name = unsafe { utf16z(&logfont.lfFaceName) };
        let name = name.trim();
        if !name.is_empty() && !name.starts_with('@') {
            // 只按名字判定：`lfCharSet` 不可靠 —— GDI 会按每个"支持"的字符集
            // 各枚举一次，Arial Unicode MS / Terminal / Fixedsys 都报 134，
            // 于是纯西文字体会跑到中文组里。
            let cjk = is_cjk_name(name);
            let ctx = unsafe { &mut *(lparam as *mut Collector<'_>) };
            match ctx
                .out
                .iter_mut()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
            {
                Some(slot) => slot.1 |= cjk,
                None => ctx.out.push((name.to_string(), cjk)),
            }
        }
    }
    1
}

struct FirstName<'a> {
    out: &'a mut Option<String>,
}

unsafe extern "system" fn first_proc(
    lf: *const ENUMLOGFONTEXW,
    _metrics: *const core::ffi::c_void,
    _kind: u32,
    lparam: isize,
) -> i32 {
    if lf.is_null() || lparam == 0 {
        return 0;
    }
    let logfont = unsafe { &(*lf).elfLogFont };
    let name = unsafe { utf16z(&logfont.lfFaceName) };
    let name = name.trim();
    if !name.is_empty() && !name.starts_with('@') {
        let ctx = unsafe { &mut *(lparam as *mut FirstName<'_>) };
        *ctx.out = Some(name.to_string());
        return 0; // 只取第一个
    }
    1
}

/// 用某个字族名反查系统里的真实名字。`SimSun` 在中文系统上回 `宋体`。
fn resolve_family(dc: HDC, id: &str) -> Option<String> {
    let mut lf = LOGFONTW::default();
    lf.lfCharSet = DEFAULT_CHARSET as u8;
    let wide: Vec<u16> = id.encode_utf16().take(31).collect();
    lf.lfFaceName[..wide.len()].copy_from_slice(&wide);
    let mut found = None;
    let mut ctx = FirstName { out: &mut found };
    unsafe {
        EnumFontFamiliesExW(
            dc,
            &lf,
            first_proc,
            &mut ctx as *mut FirstName<'_> as isize,
            0,
        );
    }
    found
}

unsafe fn utf16z(buf: &[u16]) -> String {
    let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_faces_come_first_and_labels_are_unique() {
        let list = all();
        assert!(list.len() >= font_catalog::FACES.len());
        // 默认字族必须能定位到一行，否则工具条不会显示激活态。
        assert!(index(font_catalog::DEFAULT_FACE).is_some());

        // 常用项与枚举项撞名曾是真实 bug（微软雅黑 / 微软雅黑 UI）。
        let mut labels: Vec<&str> = list.iter().map(|f| f.label.as_str()).collect();
        labels.sort_unstable();
        for w in labels.windows(2) {
            assert_ne!(w[0], w[1], "duplicate family label in the picker");
        }

        if list.iter().any(|f| is_cjk_name(&f.label)) {
            assert!(is_cjk_name(&list[0].label), "Chinese faces must come first");
        }
    }

    #[test]
    #[ignore = "diagnostic: prints every family in picker order"]
    fn dump_families() {
        for (i, f) in all().iter().enumerate() {
            println!("{i:3}  {:<34} {}", f.id, f.label);
        }
    }
}
