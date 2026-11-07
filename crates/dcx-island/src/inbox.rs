//! The unified inbox: one list for every channel, plus per-channel health.
//!
//! Nothing here talks to the network. Channels push [`InboxItem`]s in from
//! their own threads (`notify::shell_hook`, `notify::qq`, `notify::mail`) and
//! the UI thread owns the list — the same single-writer rule as tasks and the
//! clipboard.
//!
//! **Privacy boundary, enforced by the model rather than by the UI:** a channel
//! may only fill `title` with something it is allowed to show. The WeChat/WeCom
//! listener has no window title, no sender and no message body, so it reports a
//! generic line and `detail: None`. That is why the inbox does not pretend to
//! be a message list.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Qq,
    WeChat,
    WeCom,
    Mail,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Qq => "QQ",
            Source::WeChat => "微信",
            Source::WeCom => "企业微信",
            Source::Mail => "邮箱",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Source::Qq => "qq",
            Source::WeChat => "wechat",
            Source::WeCom => "wecom",
            Source::Mail => "mail",
        }
    }

    pub fn color(self) -> (u8, u8, u8) {
        match self {
            Source::Qq => (0x2B, 0x7D, 0xE9),
            Source::WeChat => (0x18, 0x9A, 0x4A),
            Source::WeCom => (0x1E, 0x8E, 0xD8),
            Source::Mail => (0xE0, 0x8A, 0x1E),
        }
    }
}

#[derive(Clone, Debug)]
pub struct InboxItem {
    pub id: u64,
    pub source: Source,
    pub title: String,
    /// Secondary line; empty when the channel may not report one.
    pub detail: String,
    pub at: u64,
    pub read: bool,
    /// Channel-side identity (notification id, message uid). Repeats are
    /// ignored so a poll every 15 s does not build 40 identical rows.
    pub key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChannelState {
    /// Not enabled by the user.
    Off,
    /// Enabled, starting up.
    Idle,
    Ok,
    /// The OS refused the permission; needs a manual action.
    Denied(String),
    /// Enabled but the mechanism is not available on this machine.
    Unavailable(String),
    Error(String),
}

impl ChannelState {
    pub fn label(&self) -> String {
        match self {
            ChannelState::Off => "未启用".to_string(),
            ChannelState::Idle => "启动中".to_string(),
            ChannelState::Ok => "已就绪".to_string(),
            ChannelState::Denied(why) => why.clone(),
            ChannelState::Unavailable(why) => why.clone(),
            ChannelState::Error(why) => format!("错误：{why}"),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, ChannelState::Ok)
    }

    pub fn is_problem(&self) -> bool {
        matches!(
            self,
            ChannelState::Denied(_) | ChannelState::Unavailable(_) | ChannelState::Error(_)
        )
    }
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub state: ChannelState,
    pub last_at: Option<u64>,
    /// Human-readable note shown under the status, e.g. "只接收任务栏关注信号".
    pub note: &'static str,
}

impl Channel {
    fn new(note: &'static str) -> Channel {
        Channel {
            state: ChannelState::Off,
            last_at: None,
            note,
        }
    }
}

pub struct Inbox {
    items: VecDeque<InboxItem>,
    next_id: u64,
    keep: usize,
    pub channels: Vec<(Source, Channel)>,
    /// Bumped on every push so the UI can request a repaint without diffing.
    pub version: u64,
}

impl Inbox {
    pub fn new(keep: usize) -> Inbox {
        Inbox {
            items: VecDeque::new(),
            next_id: 1,
            keep: keep.max(5),
            channels: vec![
                (
                    Source::WeChat,
                    Channel::new("只接收任务栏「有新消息」信号，不读取聊天内容"),
                ),
                (
                    Source::WeCom,
                    Channel::new("本地关注信号；企业微信是否触发取决于其版本"),
                ),
                (Source::Qq, Channel::new("系统通知镜像，需要通知访问权限")),
                (Source::Mail, Channel::new("IMAP 仅同步邮件头，正文按需读取")),
            ],
            version: 0,
        }
    }

    pub fn set_keep(&mut self, keep: usize) {
        self.keep = keep.max(5);
        while self.items.len() > self.keep {
            self.items.pop_back();
        }
    }

    pub fn channel(&self, source: Source) -> Option<&Channel> {
        self.channels.iter().find(|(s, _)| *s == source).map(|(_, c)| c)
    }

    pub fn set_state(&mut self, source: Source, state: ChannelState) {
        if let Some((_, ch)) = self.channels.iter_mut().find(|(s, _)| *s == source) {
            ch.state = state;
            self.version += 1;
        }
    }

    pub fn push(&mut self, source: Source, title: &str, detail: &str, key: &str) {
        if !key.is_empty() && self.items.iter().any(|i| i.source == source && i.key == key) {
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.items.push_front(InboxItem {
            id,
            source,
            title: title.to_string(),
            detail: detail.to_string(),
            at: now_secs(),
            read: false,
            key: key.to_string(),
        });
        while self.items.len() > self.keep {
            self.items.pop_back();
        }
        if let Some((_, ch)) = self.channels.iter_mut().find(|(s, _)| *s == source) {
            ch.last_at = Some(now_secs());
            if !ch.state.is_ok() && !ch.state.is_problem() {
                ch.state = ChannelState::Ok;
            }
        }
        self.version += 1;
    }

    pub fn items(&self) -> impl Iterator<Item = &InboxItem> {
        self.items.iter()
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|i| !i.read).count()
    }

    pub fn mark_all_read(&mut self) {
        for item in &mut self.items {
            item.read = true;
        }
        self.version += 1;
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.version += 1;
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|i| i.id != id);
        self.version += 1;
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `14:05` for today, `11-06` for older items.
pub fn short_time(at: u64, today_prefix: &str) -> String {
    let now = now_secs();
    let secs = now.saturating_sub(at);
    if secs < 60 {
        "刚刚".to_string()
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else if secs < 86_400 {
        format!("{} 小时前", secs / 3600)
    } else {
        // No date crate: the caller passes today's `YYYY-MM-DD` so a month-old
        // item is still rendered as a plain elapsed count instead of a wrong
        // calendar date.
        let _ = today_prefix;
        format!("{} 天前", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_keys_are_ignored() {
        let mut inbox = Inbox::new(10);
        inbox.push(Source::Mail, "新邮件", "主题", "uid-1");
        inbox.push(Source::Mail, "新邮件", "主题", "uid-1");
        assert_eq!(inbox.items().count(), 1);
        assert_eq!(inbox.unread(), 1);
    }

    #[test]
    fn the_list_is_bounded() {
        let mut inbox = Inbox::new(5);
        for i in 0..20 {
            inbox.push(Source::WeChat, "有新消息", "", &format!("k{i}"));
        }
        assert_eq!(inbox.items().count(), 5);
    }

    #[test]
    fn a_push_turns_an_idle_channel_green_but_not_a_denied_one() {
        let mut inbox = Inbox::new(10);
        inbox.set_state(Source::Mail, ChannelState::Idle);
        inbox.push(Source::Mail, "t", "", "k");
        assert!(inbox.channel(Source::Mail).unwrap().state.is_ok());

        inbox.set_state(Source::Qq, ChannelState::Denied("无权限".into()));
        inbox.push(Source::Qq, "t", "", "k2");
        assert!(inbox.channel(Source::Qq).unwrap().state.is_problem());
    }

    #[test]
    fn mark_all_read_zeroes_the_badge() {
        let mut inbox = Inbox::new(10);
        inbox.push(Source::WeCom, "t", "", "a");
        inbox.push(Source::WeCom, "t", "", "b");
        assert_eq!(inbox.unread(), 2);
        inbox.mark_all_read();
        assert_eq!(inbox.unread(), 0);
    }
}
