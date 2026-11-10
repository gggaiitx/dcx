//! Windows 字体目录：截图标注（`dcx-snip`）与设置中心（`dcx-host`）共用的
//! 可选字族。
//!
//! 只列 Windows 10/11 自带、且覆盖 CJK 的字族。`id` 是直接交给 `CreateFontW`
//! 的 `lfFaceName`，`label` 是工具条与设置页显示的名字。两边必须读同一份表：
//! 设置页写进 `snip.toml` 的字符串，正是 `dcx-snip` 拿去建字体的字符串，各写
//! 一份常量迟早会漂移。
//!
//! 字族不在本机时 GDI 会**静默**替换成默认字体（不会失败），所以这里只收
//! 系统自带项；用户手工写进 TOML 的其他名字仍然可用，只是工具条不保证能
//! 显示其中文名。

/// 一个字族：`id` = GDI 字族名，`label` = 界面显示名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    pub id: &'static str,
    pub label: &'static str,
}

/// 工具条与设置页共用的字族顺序（第一项即默认值）。
#[rustfmt::skip]
pub const FACES: [Face; 7] = [
    Face { id: "Microsoft YaHei", label: "微软雅黑" },
    Face { id: "SimHei", label: "黑体" },
    Face { id: "SimSun", label: "宋体" },
    Face { id: "KaiTi", label: "楷体" },
    Face { id: "FangSong", label: "仿宋" },
    Face { id: "DengXian", label: "等线" },
    Face { id: "Segoe UI", label: "西文" },
];

/// 新建标注文本的默认字族。
pub const DEFAULT_FACE: &str = FACES[0].id;

/// 显示名；不在目录里的字族原样返回（TOML 允许任意名字）。
pub fn label(id: &str) -> &str {
    index(id).map(|i| FACES[i].label).unwrap_or(id)
}

/// 目录下标；大小写不敏感，因为 TOML 里的手写值不受控件约束。
pub fn index(id: &str) -> Option<usize> {
    let id = id.trim();
    FACES.iter().position(|f| f.id.eq_ignore_ascii_case(id))
}

/// 工具条按钮里的短名：目录项用中文名，其他字族按字符截到 `max` 个。
pub fn short(id: &str, max: usize) -> &str {
    match index(id) {
        Some(i) => FACES[i].label,
        None => {
            let end = id
                .char_indices()
                .nth(max)
                .map(|(i, _)| i)
                .unwrap_or(id.len());
            id[..end].trim()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lookup_is_case_insensitive() {
        assert_eq!(index("microsoft yahei"), Some(0));
        assert_eq!(index(DEFAULT_FACE), Some(0));
        assert_eq!(label("SimSun"), "宋体");
        assert_eq!(label("Some Custom Face"), "Some Custom Face");
        assert!(DEFAULT_FACE == FACES[0].id);
    }

    #[test]
    fn short_never_splits_a_char() {
        // 中文按字节截断会 panic；`char_indices` 保证边界。
        assert_eq!(short("微软雅黑自定义字体", 3), "微软雅");
        assert_eq!(short("微软雅黑", 8), "微软雅黑");
    }
}
