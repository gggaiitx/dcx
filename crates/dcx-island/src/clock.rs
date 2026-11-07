//! Wall clock from `GetLocalTime`.
//!
//! No `chrono`: the system already knows the time zone, the daylight-saving
//! rules and the user's 12/24-hour preference, and a crate would replace all
//! three with guesses. The only thing this module owns is formatting.

/// One local time reading, plus the weekday derived from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub weekday: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

impl Clock {
    pub fn now() -> Clock {
        let mut st = dcx_win32::SYSTEMTIME::default();
        unsafe { dcx_win32::GetLocalTime(&mut st) };
        Clock {
            year: st.year,
            month: st.month,
            day: st.day,
            weekday: st.day_of_week,
            hour: st.hour,
            minute: st.minute,
            second: st.second,
            millis: st.milliseconds,
        }
    }

    /// `14:05` or `2:05 PM`, optionally with seconds.
    pub fn time_text(&self, hour24: bool, seconds: bool) -> String {
        let (h, suffix) = if hour24 {
            (self.hour, "")
        } else {
            let h = self.hour % 12;
            (if h == 0 { 12 } else { h }, if self.hour < 12 { " AM" } else { " PM" })
        };
        if seconds {
            format!("{:02}:{:02}:{:02}{}", h, self.minute, self.second, suffix)
        } else if hour24 {
            format!("{:02}:{:02}", h, self.minute)
        } else {
            format!("{}:{:02}{}", h, self.minute, suffix)
        }
    }

    pub fn weekday_cn(&self) -> &'static str {
        match self.weekday {
            0 => "周日",
            1 => "周一",
            2 => "周二",
            3 => "周三",
            4 => "周四",
            5 => "周五",
            _ => "周六",
        }
    }

    /// `11月7日` — used in the panel header.
    pub fn date_cn(&self) -> String {
        format!("{}月{}日", self.month, self.day)
    }

    /// `2026-11-07` — used for archives and task buckets.
    pub fn iso_date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `HH:MM` in 24-hour form, for logs and reminder text.
    pub fn hhmm(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// Whole seconds since local midnight. Task due times are stored as this
    /// number, so they stay comparable without a time-zone type.
    pub fn secs_of_day(&self) -> u32 {
        self.hour as u32 * 3600 + self.minute as u32 * 60 + self.second as u32
    }

    /// Milliseconds until the next whole second, clamped so a slow frame cannot
    /// be asked to sleep for a whole second.
    pub fn to_next_second(&self) -> std::time::Duration {
        let ms = 1000u32.saturating_sub(self.millis as u32).max(16);
        std::time::Duration::from_millis(ms as u64)
    }
}

/// Split a `HH:MM` string into seconds of day. `None` for anything malformed —
/// the task editor shows an inline error instead of silently storing 00:00.
pub fn parse_hhmm(text: &str) -> Option<u32> {
    let (h, m) = text.trim().split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 3600 + m * 60)
}

pub fn hhmm_of(secs: u32) -> String {
    let s = secs % 86_400;
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u16, minute: u16) -> Clock {
        Clock {
            year: 2026,
            month: 11,
            day: 7,
            weekday: 6,
            hour,
            minute,
            second: 5,
            millis: 250,
        }
    }

    #[test]
    fn formats_24_and_12_hour_clocks() {
        assert_eq!(at(14, 5).time_text(true, false), "14:05");
        assert_eq!(at(14, 5).time_text(false, false), "2:05 PM");
        assert_eq!(at(0, 9).time_text(false, false), "12:09 AM");
        assert_eq!(at(0, 9).time_text(true, true), "00:09:05");
        assert_eq!(at(12, 0).time_text(false, false), "12:00 PM");
    }

    #[test]
    fn weekday_and_date_are_chinese() {
        assert_eq!(at(9, 0).weekday_cn(), "周六");
        assert_eq!(at(9, 0).date_cn(), "11月7日");
        assert_eq!(at(9, 0).iso_date(), "2026-11-07");
    }

    #[test]
    fn seconds_of_day_round_trips_through_hhmm() {
        let secs = parse_hhmm("15:30").unwrap();
        assert_eq!(secs, 15 * 3600 + 30 * 60);
        assert_eq!(hhmm_of(secs), "15:30");
        assert!(parse_hhmm("24:00").is_none());
        assert!(parse_hhmm("12:60").is_none());
        assert!(parse_hhmm("noon").is_none());
    }

    #[test]
    fn next_second_is_never_zero() {
        assert!(at(1, 1).to_next_second().as_millis() >= 16);
        let full = Clock {
            millis: 999,
            ..at(1, 1)
        };
        assert!(full.to_next_second().as_millis() >= 1);
    }
}
