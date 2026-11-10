//! Throwaway diagnostic: prints what GSMTC actually reports, so a player whose
//! `Position` never advances — or an app that publishes no session at all — can
//! be identified. Not part of the app.
//!
//! ```text
//! cargo run -p dcx-island --example media_probe          # timeline of the current session
//! cargo run -p dcx-island --example media_probe -- --all # every registered session
//! ```

use std::time::Duration;

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession,
    GlobalSystemMediaTransportControlsSessionManager,
};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

fn tick(v: Option<windows::Foundation::TimeSpan>) -> f64 {
    v.map(|v| v.Duration as f64 / 1e7).unwrap_or(-1.0)
}

fn describe(s: &GlobalSystemMediaTransportControlsSession, tag: &str) {
    let aumid = s
        .SourceAppUserModelId()
        .map(|h| h.to_string())
        .unwrap_or_else(|_| "?".into());
    let title = s
        .TryGetMediaPropertiesAsync()
        .ok()
        .and_then(|o| o.join().ok())
        .and_then(|p| p.Title().ok())
        .map(|h| h.to_string())
        .unwrap_or_default();
    let info = s.GetPlaybackInfo().ok();
    let st = info
        .as_ref()
        .and_then(|i| i.PlaybackStatus().ok())
        .map(|x| {
            format!("{x:?}").replace(
                "GlobalSystemMediaTransportControlsSessionPlaybackStatus",
                "",
            )
        })
        .unwrap_or_else(|| "-".into());
    let rate = info
        .as_ref()
        .and_then(|i| i.PlaybackRate().ok())
        .and_then(|r| r.Value().ok());
    let tl = s.GetTimelineProperties().ok();
    let start = tick(tl.as_ref().and_then(|t| t.StartTime().ok()));
    let pos = tick(tl.as_ref().and_then(|t| t.Position().ok()));
    let end = tick(tl.as_ref().and_then(|t| t.EndTime().ok()));
    println!(
        "{tag} aumid={aumid}\n   status={st} rate={rate:?} pos={pos:8.3} start={start:6.3} end={end:8.3}  {title}"
    );
}

fn main() {
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let all = std::env::args().any(|a| a == "--all");
    let mgr = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|o| o.join())
    {
        Ok(m) => m,
        Err(e) => {
            println!("manager unavailable: {e}");
            return;
        }
    };

    if all {
        let sessions = match mgr.GetSessions() {
            Ok(v) => v,
            Err(e) => {
                println!("GetSessions failed: {e}");
                return;
            }
        };
        let n = sessions.Size().unwrap_or(0);
        let current = mgr
            .GetCurrentSession()
            .ok()
            .and_then(|s| s.SourceAppUserModelId().ok())
            .map(|h| h.to_string())
            .unwrap_or_else(|| "(none)".into());
        println!("sessions={n} current={current}");
        for i in 0..n {
            if let Ok(s) = sessions.GetAt(i) {
                describe(&s, &format!("[{i}]"));
            }
        }
        return;
    }

    for i in 0..15 {
        match mgr.GetCurrentSession() {
            Ok(s) => describe(&s, &format!("{i:02}")),
            Err(e) => println!("{i:02} no session: {e}"),
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
}
