//! Outbound delivery: task reminders pushed to WeCom or by mail.
//!
//! # Boundaries
//!
//! * **WeCom group robot** — the only officially supported way for a desktop
//!   program to send to WeCom without a server. Rate limit is 20 messages per
//!   minute per bot; `min_gap_secs` keeps us far below it, and a failure is
//!   retried at most once per reminder.
//! * **SMTP** — same account as the inbound IMAP, separate credential.
//! * **Personal QQ / WeChat — not implemented, by design.** Neither has an
//!   official send API for a desktop app. Protocol reverse-engineering and UI
//!   automation are both out of scope: they break silently on every client
//!   update and violate the terms of service. The settings card says so instead
//!   of offering a switch that cannot work.
//!
//! Secrets never appear in a TOML or a log: only the credential-manager keys
//! `wecom_key` and `mail_password` are read.

use std::time::{Duration, Instant};

use dcx_island_config::Config;

use crate::sys;

// Same names the settings centre writes through the shared credential module.
pub use dcx_island_config::credential::{
    KEY_IMAP as CRED_IMAP, KEY_MAIL as CRED_MAIL, KEY_WECOM as CRED_WECOM,
};

#[derive(Default)]
pub struct Dispatcher {
    last: Option<Instant>,
    /// Last error per channel, for the settings card.
    pub last_error: Option<String>,
    pub last_ok: Option<String>,
}

impl Dispatcher {
    /// Send `text` to every enabled channel. Returns the channels that accepted
    /// the message; an error string lists what failed.
    pub fn push(&mut self, cfg: &Config, text: &str) -> Result<Vec<&'static str>, String> {
        if !cfg.dispatch.wecom && !cfg.dispatch.mail {
            return Err("未启用任何推送渠道".to_string());
        }
        let gap = Duration::from_secs(cfg.dispatch.min_gap_secs);
        if let Some(last) = self.last {
            if last.elapsed() < gap {
                return Err(format!(
                    "发送过于频繁，请等待 {} 秒",
                    (gap - last.elapsed()).as_secs() + 1
                ));
            }
        }
        self.last = Some(Instant::now());

        let mut sent = Vec::new();
        let mut errors = Vec::new();

        if cfg.dispatch.wecom {
            match sys::cred_read(CRED_WECOM) {
                Some(key) if !key.trim().is_empty() => match push_wecom(&key, text) {
                    Ok(()) => sent.push("企业微信"),
                    Err(e) => errors.push(format!("企业微信：{e}")),
                },
                _ => errors.push("企业微信：未保存 Webhook Key".to_string()),
            }
        }

        if cfg.dispatch.mail {
            let password = sys::cred_read(CRED_MAIL)
                .or_else(|| sys::cred_read(CRED_IMAP))
                .unwrap_or_default();
            if cfg.mail.user.trim().is_empty() || password.is_empty() {
                errors.push("邮箱：未配置账号或授权码".to_string());
            } else {
                match crate::notify::mail::send_mail(
                    &cfg.mail.smtp_host,
                    cfg.mail.smtp_port,
                    &cfg.mail.user,
                    &password,
                    &cfg.mail.to,
                    "桌面信息岛提醒",
                    text,
                ) {
                    Ok(_) => sent.push("邮箱"),
                    Err(e) => errors.push(format!("邮箱：{e}")),
                }
            }
        }

        if errors.is_empty() {
            self.last_ok = Some(now_label());
            self.last_error = None;
            Ok(sent)
        } else {
            self.last_error = Some(errors.join("；"));
            if sent.is_empty() {
                Err(errors.join("；"))
            } else {
                Ok(sent)
            }
        }
    }
}

fn now_label() -> String {
    let c = crate::clock::Clock::now();
    format!("{} {}", c.date_cn(), c.hhmm())
}

/// One-shot send with no rate-limit state of its own.
///
/// The caller (the UI thread) owns the gap between messages, because the send
/// itself runs on a worker thread: a webhook that takes 5 s to time out must
/// never block a frame.
pub fn push_once(cfg: &Config, text: &str) -> Result<Vec<&'static str>, String> {
    let mut dispatcher = Dispatcher::default();
    dispatcher.push(cfg, text)
}

/// POST one text message to a WeCom group robot.
pub fn push_wecom(key: &str, text: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Webhook Key 为空".to_string());
    }
    let url = format!("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key={key}");
    let payload = serde_json::json!({
        "msgtype": "text",
        "text": { "content": text }
    })
    .to_string();
    let (code, body) = sys::https_post(&url, "application/json", &payload)?;
    if code != 200 {
        return Err(format!("HTTP {code}"));
    }
    // The API answers 200 with `{"errcode":93000,...}` for a bad key; the
    // status code alone would report success.
    if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&body) {
        let errcode = doc.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
        if errcode != 0 {
            let msg = doc
                .get("errmsg")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误");
            return Err(format!("errcode {errcode}：{msg}"));
        }
    }
    Ok(())
}

/// Format one reminder for delivery.
pub fn reminder_text(title: &str, at: Option<u32>) -> String {
    let clock = crate::clock::Clock::now();
    match at {
        Some(secs) => format!(
            "【提醒】{}\n时间：{} {}",
            title,
            clock.date_cn(),
            crate::clock::hhmm_of(secs)
        ),
        None => format!("【提醒】{}\n时间：{}", title, clock.date_cn()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_text_carries_the_title_and_time() {
        let text = reminder_text("交周报", Some(15 * 3600 + 30 * 60));
        assert!(text.contains("交周报"));
        assert!(text.contains("15:30"));
    }

    #[test]
    fn a_disabled_dispatcher_refuses_instead_of_silently_dropping() {
        let cfg = Config::default();
        let mut d = Dispatcher::default();
        assert!(d.push(&cfg, "x").is_err());
    }
}
