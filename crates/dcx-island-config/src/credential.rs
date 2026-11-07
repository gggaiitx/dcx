//! Windows 凭据管理器（Credential Manager）读写。
//!
//! 授权码 / Webhook Key **只存在这里**：不进 `island.toml`、不进日志、不进
//! 截图。两个进程都要访问同一批目标名，所以这段 FFI 放在共享 crate 里，
//! 而不是各自复制一份——目标名字符串一旦漂移，设置中心保存的凭据会永远
//! 读不出来，且没有任何报错。
//!
//! 目标名固定为 `DCX/island/<key>`，`CRED_TYPE_GENERIC` +
//! `CRED_PERSIST_LOCAL_MACHINE`，与用户账户绑定。

use core::ffi::c_void;

/// 企业微信群机器人 Webhook 的 `key=` 参数。
pub const KEY_WECOM: &str = "wecom_key";
/// SMTP 发件授权码（可与 IMAP 授权码不同）。
pub const KEY_MAIL: &str = "mail_password";
/// IMAP 收件授权码；未单独配置 SMTP 密码时复用它。
pub const KEY_IMAP: &str = "imap_password";

const CRED_TYPE_GENERIC: u32 = 1;
const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;

type Bool = i32;

#[repr(C)]
struct CredentialW {
    flags: u32,
    kind: u32,
    target_name: *mut u16,
    comment: *mut u16,
    last_written: dcx_win32::FILETIME,
    credential_blob_size: u32,
    credential_blob: *mut u8,
    persist: u32,
    attribute_count: u32,
    attributes: *mut c_void,
    target_alias: *mut u16,
    user_name: *mut u16,
}

#[link(name = "advapi32")]
extern "system" {
    fn CredWriteW(cred: *const CredentialW, flags: u32) -> Bool;
    fn CredReadW(target: *const u16, kind: u32, flags: u32, out: *mut *mut CredentialW) -> Bool;
    fn CredFree(buf: *mut c_void);
    fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> Bool;
}

fn target(key: &str) -> Vec<u16> {
    dcx_win32::wide(&format!("DCX/island/{key}"))
}

/// 保存（覆盖）一条凭据。空字符串等价于删除，避免留一条「有但为空」的
/// 记录让诊断卡片误报「已配置」。
pub fn save(key: &str, secret: &str) -> Result<(), String> {
    if secret.is_empty() {
        delete(key);
        return Ok(());
    }
    let mut name = target(key);
    let mut user = dcx_win32::wide("dcx-island");
    let mut blob = secret.as_bytes().to_vec();
    let cred = CredentialW {
        flags: 0,
        kind: CRED_TYPE_GENERIC,
        target_name: name.as_mut_ptr(),
        comment: core::ptr::null_mut(),
        last_written: dcx_win32::FILETIME::default(),
        credential_blob_size: blob.len() as u32,
        credential_blob: blob.as_mut_ptr(),
        persist: CRED_PERSIST_LOCAL_MACHINE,
        attribute_count: 0,
        attributes: core::ptr::null_mut(),
        target_alias: core::ptr::null_mut(),
        user_name: user.as_mut_ptr(),
    };
    if unsafe { CredWriteW(&cred, 0) } == 0 {
        return Err(format!(
            "写入凭据失败（错误码 {}）",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        ));
    }
    Ok(())
}

/// 读一条凭据；不存在或为空都返回 `None`。绝不把内容写进日志。
pub fn read(key: &str) -> Option<String> {
    let name = target(key);
    let mut out: *mut CredentialW = core::ptr::null_mut();
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut out) } == 0 || out.is_null() {
        return None;
    }
    let text = unsafe {
        let cred = &*out;
        let bytes = core::slice::from_raw_parts(
            cred.credential_blob,
            cred.credential_blob_size as usize,
        );
        let s = String::from_utf8_lossy(bytes).to_string();
        CredFree(out as *mut c_void);
        s
    };
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub fn exists(key: &str) -> bool {
    read(key).is_some()
}

pub fn delete(key: &str) {
    let name = target(key);
    unsafe {
        CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0);
    }
}
