//! Notification channels: one module per mechanism, one poll loop for all.
//!
//! Each channel owns its thread and channel; [`Notifier::poll`] drains them on
//! the UI thread. A channel that cannot start reports a [`ChannelState`] and is
//! never retried in a tight loop — the island must stay usable with zero
//! channels configured, which is the default.

pub mod mail;
pub mod qq;
pub mod shell_hook;

use dcx_island_config::Config;

use crate::inbox::{ChannelState, Inbox, Source};

pub struct Notifier {
    shell: Option<shell_hook::ShellHook>,
    qq: Option<qq::QqProbe>,
    mail: Option<mail::MailProbe>,
}

impl Notifier {
    pub fn start(cfg: &Config, mail_password: Option<String>) -> Notifier {
        let mut shell = None;
        let mut qq = None;
        let mut mail_probe = None;

        if cfg.inbox.wechat || cfg.inbox.wecom {
            shell = Some(shell_hook::start());
        }
        if cfg.inbox.qq || cfg.inbox.system {
            qq = qq::start(cfg.inbox.qq, cfg.inbox.system);
        }
        if cfg.inbox.mail {
            if let Some(password) = mail_password {
                mail_probe = mail::start(mail::MailSettings {
                    host: cfg.mail.imap_host.clone(),
                    port: cfg.mail.imap_port,
                    user: cfg.mail.user.clone(),
                    password,
                    poll_secs: cfg.mail.poll_minutes * 60,
                    skip_backlog: cfg.mail.skip_backlog,
                });
            }
        }

        Notifier {
            shell,
            qq,
            mail: mail_probe,
        }
    }

    /// Fill the channel cards once, before the first poll.
    pub fn seed_states(&self, inbox: &mut Inbox, cfg: &Config) {
        use ChannelState::*;
        // WeChat / WeCom share one hook: if it started at all, the mechanism is
        // armed; whether the app actually flashes is reported by `last_at`.
        let hook_ok = self.shell.is_some();
        if cfg.inbox.wechat {
            inbox.set_state(Source::WeChat, if hook_ok { Ok } else { Unavailable("无法启动任务栏监听".into()) });
        } else {
            inbox.set_state(Source::WeChat, Off);
        }
        if cfg.inbox.wecom {
            inbox.set_state(Source::WeCom, if hook_ok { Ok } else { Unavailable("无法启动任务栏监听".into()) });
        } else {
            inbox.set_state(Source::WeCom, Off);
        }
        if !cfg.inbox.qq {
            inbox.set_state(Source::Qq, Off);
        } else if self.qq.is_none() {
            inbox.set_state(Source::Qq, Unavailable("通知监听线程未能启动".into()));
        }
        if !cfg.inbox.system {
            inbox.set_state(Source::System, Off);
        } else if self.qq.is_none() {
            inbox.set_state(Source::System, Unavailable("通知监听线程未能启动".into()));
        }
        if !cfg.inbox.mail {
            inbox.set_state(Source::Mail, Off);
        } else if self.mail.is_none() {
            inbox.set_state(
                Source::Mail,
                if cfg.mail.user.trim().is_empty() {
                    Unavailable("未填写邮箱账号".into())
                } else {
                    Unavailable("未保存授权码".into())
                },
            );
        }
    }

    /// Drain every channel. Returns true when the inbox changed.
    pub fn poll(&mut self, inbox: &mut Inbox) -> bool {
        let before = inbox.version;

        if let Some(shell) = &self.shell {
            while let Ok(flash) = shell.rx.try_recv() {
                // No app name in the title: the inbox row already prints the
                // source on its own line, and "微信：有新消息" under a "微信"
                // caption read as a duplicated line.
                inbox.push(
                    flash.source,
                    "有新消息",
                    "",
                    &format!("flash-{}-{}", flash.source.key(), crate::inbox::now_secs() / 3),
                );
            }
        }

        if let Some(qq) = &self.qq {
            while let Ok(hit) = qq.rx.try_recv() {
                // QQ keeps its own channel while its switch is on; with the
                // switch off (system-only mode) a QQ toast is just another
                // app's row.
                let source = if hit.is_qq && qq.qq_enabled {
                    Source::Qq
                } else {
                    Source::System
                };
                let title = if hit.title.trim().is_empty() {
                    if source == Source::Qq {
                        "QQ 新消息".to_string()
                    } else {
                        format!("{} 通知", hit.app)
                    }
                } else {
                    hit.title.clone()
                };
                inbox.push_with_app(
                    source,
                    &hit.app,
                    &title,
                    &hit.body,
                    &format!("toast-{}", hit.id),
                );
            }
            // One listener, two channels: mirror its health to whichever
            // switches are on. The other stays `Off` from `seed_states`.
            if let Ok(state) = qq.state.lock() {
                for source in [Source::Qq, Source::System] {
                    let enabled = match source {
                        Source::Qq => qq.qq_enabled,
                        _ => qq.system_enabled,
                    };
                    if !enabled {
                        continue;
                    }
                    if let Some(ch) = inbox.channel(source) {
                        if ch.state != *state {
                            inbox.set_state(source, state.clone());
                        }
                    }
                }
            }
        }

        if let Some(mail) = &self.mail {
            while let Ok(hit) = mail.rx.try_recv() {
                let title = if hit.subject.trim().is_empty() {
                    "新邮件".to_string()
                } else {
                    hit.subject.clone()
                };
                inbox.push(
                    Source::Mail,
                    &title,
                    &hit.from,
                    &format!("mail-{}", hit.uid),
                );
            }
            if let Ok(state) = mail.state.lock() {
                if let Some(ch) = inbox.channel(Source::Mail) {
                    if ch.state != *state {
                        inbox.set_state(Source::Mail, state.clone());
                    }
                }
            }
        }

        inbox.version != before
    }
}

/// Human summary for the diagnostics card: "2 / 4 渠道就绪".
pub fn readiness(inbox: &Inbox) -> (usize, usize) {
    let mut enabled = 0;
    let mut ok = 0;
    for (_, ch) in &inbox.channels {
        if !matches!(ch.state, ChannelState::Off) {
            enabled += 1;
            if ch.state.is_ok() {
                ok += 1;
            }
        }
    }
    (ok, enabled)
}
