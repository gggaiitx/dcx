//! 「开机自启」开关。
//!
//! 走 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 而不是
//! `shell:startup` 里的 .lnk：一个字符串值就够，不需要 COM/IShellLink，
//! 也不会在用户的启动文件夹里留下一个需要解释的文件。
//!
//! 命令行固定带 `--no-settings`：开机只进托盘，不弹设置窗口——这也是
//! `说明.txt` 里给快捷方式推荐的那个参数。

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "DCX";
const ARGS: &str = " --no-settings";

/// 本程序的可执行文件路径（`current_exe` 在 Windows 上已经是绝对路径，
/// 且不带 `\\?\` 前缀之外的花样，直接写进启动项即可）。
fn exe_path() -> Option<String> {
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
}

/// 写进启动项的完整命令行。
fn command() -> Option<String> {
    exe_path().map(|exe| format!("\"{exe}\"{ARGS}"))
}

/// Executable path out of a Run-value command line.
///
/// A quoted path may contain spaces; an unquoted one ends at the first space.
fn exe_of_command(value: &str) -> Option<String> {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('"') {
        rest.split('"').next().map(|s| s.trim().to_string())
    } else {
        v.split_whitespace().next().map(str::to_string)
    }
    .filter(|s| !s.is_empty())
}

/// 启动项里的这条命令是否指的就是"当前这个 exe"。
///
/// 只比可执行文件路径，不管后面的参数：用户手工加过 `--no-settings` 也好、
/// 没有也好，都算已开启，重新打开开关时会补成我们推荐的形式。
///
/// The comparison is on the **whole** executable path. A substring test made any
/// value that merely mentioned our path anywhere (an argument, a comment, a
/// different exe under a directory whose name contains ours) read as "enabled",
/// so the switch showed ON while nothing started at logon.
fn points_at_us(value: &str) -> bool {
    let Some(exe) = exe_path() else {
        return false;
    };
    match exe_of_command(value) {
        Some(found) => found
            .trim_end_matches(['\\', '/'])
            .eq_ignore_ascii_case(exe.trim_end_matches(['\\', '/'])),
        None => false,
    }
}

/// 开关当前状态。启动项存在但指向旧路径（便携目录被搬走）时报 false，
/// 因为那一条开机时并不会启动本程序。
pub fn is_enabled() -> bool {
    dcx_win32::reg_read_string(RUN_KEY, VALUE_NAME)
        .map(|v| points_at_us(&v))
        .unwrap_or(false)
}

/// 打开/关闭。错误直接带上 `LSTATUS`，方便在设置窗口底部照抄给用户。
pub fn set_enabled(on: bool) -> Result<(), String> {
    if on {
        let cmd = command().ok_or_else(|| "读不到本程序路径".to_owned())?;
        dcx_win32::reg_write_string(RUN_KEY, VALUE_NAME, &cmd)
            .map_err(|rc| format!("写入启动项失败（0x{rc:08X}）"))
    } else {
        dcx_win32::reg_delete_value(RUN_KEY, VALUE_NAME)
            .map_err(|rc| format!("删除启动项失败（0x{rc:08X}）"))
    }
}

/// 启动时自愈：便携包整个目录被搬走之后，注册表里留着旧路径，用户会以为
/// 自启还开着，其实开机什么都没发生。这里把旧路径改成当前路径。
///
/// 只在"值存在但指错地方"时动手；用户主动关闭过（值不存在）就不去复活它。
pub fn refresh_path() {
    let Some(have) = dcx_win32::reg_read_string(RUN_KEY, VALUE_NAME) else {
        return;
    };
    if points_at_us(&have) {
        return;
    }
    if let Some(cmd) = command() {
        let _ = dcx_win32::reg_write_string(RUN_KEY, VALUE_NAME, &cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_quoted_and_quiet() {
        // 路径含空格时 Windows 要求整体加引号；开机自启必须带 --no-settings，
        // 否则登录瞬间会弹设置窗口。
        let cmd = command().expect("current_exe");
        assert!(cmd.starts_with('"'), "命令行要以引号开头: {cmd}");
        assert!(cmd.contains(&format!("\"{ARGS}")), "要带 --no-settings: {cmd}");
    }

    #[test]
    fn points_at_us_only_for_our_own_path() {
        let ours = command().expect("current_exe");
        assert!(points_at_us(&ours));
        assert!(points_at_us(&ours.to_uppercase()), "Windows 路径不分大小写");
        assert!(!points_at_us(r#""C:\Somewhere\Else\dcx-host.exe" --no-settings"#));
        // Our path only appearing *inside* the command (as an argument, not as
        // the executable) must not read as "enabled": the old substring test
        // said ON while nothing started at logon.
        assert!(!points_at_us(&format!(
            r#""C:\Somewhere\Else\launcher.exe" {ours}"#
        )));
    }
}
