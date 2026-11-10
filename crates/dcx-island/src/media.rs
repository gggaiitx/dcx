//! The system media session (GSMTC): what is playing, the three transport
//! keys, and a locally interpolated play position.
//!
//! Every WinRT call here blocks, so they all run on this module's thread; the
//! UI only ever drains a channel. Session events (properties, playback,
//! timeline) flip a dirty flag; a 1 s poll covers players that raise no events
//! and keeps the position fresh. Cover art is decoded here — the UI thread
//! only builds the texture.
//!
//! v1 accepts one limitation: a player that publishes no GSMTC session (some
//! old players) simply does not appear; there is no window-title fallback.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession,
    GlobalSystemMediaTransportControlsSessionMediaProperties,
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

/// Fallback sampling cadence; events short-circuit it.
const POLL: Duration = Duration::from_secs(1);
/// How long the thread sleeps between command checks. Also the worst-case
/// latency between a session event and the resample it triggers.
const TICK: Duration = Duration::from_millis(250);
/// The panel draws the cover at 34 pt; 256 px is plenty at 200 % scaling.
const COVER_MAX: u32 = 256;
const COVER_MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Prev,
    Toggle,
    Next,
}

#[derive(Clone, Debug)]
pub struct Cover {
    /// Content hash, so the UI rebuilds its texture only when the art changes
    /// rather than on every one-second snapshot.
    pub id: u64,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub can_prev: bool,
    pub can_toggle: bool,
    pub can_next: bool,
    /// Seconds into the track at the moment the snapshot was taken.
    pub position: f64,
    /// Track length in seconds; 0 when the player reports none.
    pub duration: f64,
    /// Playback rate (podcasts); 1.0 when unreported.
    pub rate: f64,
    /// When `position` was read, on the monotonic clock the UI shares. The UI
    /// interpolates from this instant rather than from when the message was
    /// drained: the drain runs on a 200 ms tick and that lag would otherwise
    /// look like a 200 ms timeline disagreement.
    pub at: Instant,
    /// Shared, not cloned: a snapshot is sent every second and the RGBA
    /// buffer may be a quarter of a megabyte.
    pub cover: Option<Arc<Cover>>,
}

pub enum Msg {
    Now(Option<Snapshot>),
    Error(String),
}

pub struct Media {
    pub rx: Receiver<Msg>,
    cmd: Sender<Cmd>,
    stop: Arc<AtomicBool>,
}

impl Drop for Media {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Media {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmd.send(cmd);
    }
}

pub fn start(enabled: bool) -> Option<Media> {
    if !enabled {
        return None;
    }
    let (tx, rx) = channel();
    let (cmd_tx, cmd_rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("dcx-island-media".to_string())
        .spawn(move || run(tx, cmd_rx, stop2))
        .ok()?;
    Some(Media {
        rx,
        cmd: cmd_tx,
        stop,
    })
}

// ---------------------------------------------------------------------------
// UI-side view
// ---------------------------------------------------------------------------

/// How far the player's report may sit from the local clock before the clock
/// is pulled over to it. Below this the two disagree only by sampling lag, and
/// jumping would make the bar step backwards once per poll.
const SYNC_TOL: f64 = 1.25;

pub struct NowPlaying {
    snap: Option<Snapshot>,
    cover_tex: Option<(u64, egui::TextureHandle)>,
    /// The local clock: the position drawn is `base + (now - base_at) × rate`.
    ///
    /// It is deliberately *not* reset on every snapshot. Most players publish
    /// a GSMTC timeline only on state changes, and a few publish it once per
    /// track, so re-basing every second would pin the position to a stale
    /// value — the progress that "does not move".
    base: f64,
    base_at: Instant,
    rate: f64,
    playing: bool,
    /// Track identity `base` belongs to; a change re-bases the clock.
    key: String,
    /// The raw position the player last reported, used to tell a frozen
    /// timeline from a real seek.
    reported: Option<f64>,
}

impl NowPlaying {
    pub fn new() -> Self {
        NowPlaying {
            snap: None,
            cover_tex: None,
            base: 0.0,
            base_at: Instant::now(),
            rate: 1.0,
            playing: false,
            key: String::new(),
            reported: None,
        }
    }

    /// Where the clock would be at `at`, without consulting the player.
    fn estimate(&self, at: Instant) -> f64 {
        if !self.playing {
            return self.base;
        }
        let dt = at.saturating_duration_since(self.base_at).as_secs_f64();
        (self.base + dt * self.rate).max(0.0)
    }

    pub fn set(&mut self, ctx: &egui::Context, snap: Option<Snapshot>) {
        if let Some(c) = snap.as_ref().and_then(|s| s.cover.as_ref()) {
            let stale = self
                .cover_tex
                .as_ref()
                .map(|(id, _)| *id != c.id)
                .unwrap_or(true);
            if stale {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [c.w as usize, c.h as usize],
                    &c.rgba,
                );
                self.cover_tex = Some((
                    c.id,
                    ctx.load_texture(format!("media-cover-{}", c.id), img, Default::default()),
                ));
            }
        } else {
            self.cover_tex = None;
        }
        self.apply(snap);
    }

    /// The clock half of `set`, split out so it can be exercised without an
    /// egui context.
    fn apply(&mut self, snap: Option<Snapshot>) {
        let key = snap.as_ref().map(track_key).unwrap_or_default();
        match snap.as_ref() {
            Some(s) => {
                let rate = if s.rate > 0.0 { s.rate } else { 1.0 };
                // Where the clock had reached by the instant the player was
                // read — the only fair comparison against `s.position`.
                let est = self.estimate(s.at);
                let moved = match self.reported {
                    Some(prev) => (s.position - prev).abs() > 0.05,
                    None => true,
                };
                let adopt = self.snap.is_none()          // first snapshot
                    || key != self.key                   // new track
                    || (rate - self.rate).abs() > 0.01   // speed changed
                    || (moved && (s.position - est).abs() > SYNC_TOL) // seek
                    // The player is ahead of the clock (resume from sleep, or
                    // a timeline that only ticks now and then): take its word.
                    || s.position > est + 1.0;
                self.base = if adopt { s.position } else { est };
                self.base_at = s.at;
                self.rate = rate;
                self.playing = s.playing;
                self.key = key;
                self.reported = Some(s.position);
            }
            None => {
                self.base = 0.0;
                self.base_at = Instant::now();
                self.rate = 1.0;
                self.playing = false;
                self.key.clear();
                self.reported = None;
            }
        }
        self.snap = snap;
    }

    pub fn snap(&self) -> Option<&Snapshot> {
        self.snap.as_ref()
    }

    pub fn cover(&self) -> Option<&egui::TextureHandle> {
        self.cover_tex.as_ref().map(|(_, t)| t)
    }

    /// The position to draw: the local clock, corrected by the player only
    /// when it reports something the clock cannot explain by itself.
    pub fn position(&self) -> f64 {
        let Some(s) = &self.snap else { return 0.0 };
        let mut pos = self.estimate(Instant::now());
        if s.duration > 0.0 {
            pos = pos.min(s.duration);
        }
        pos.max(0.0)
    }

    /// Track title for the capsule — playing only, and empty titles are
    /// nothing to show.
    pub fn capsule_title(&self) -> Option<String> {
        let s = self.snap.as_ref()?;
        if !s.playing || s.title.is_empty() {
            return None;
        }
        Some(s.title.clone())
    }
}

/// Identity of "the same playback". A title/artist/album/duration change is a
/// new track; the clock restarts from whatever position it reports.
fn track_key(s: &Snapshot) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{:.3}",
        s.title, s.artist, s.album, s.duration
    )
}

// ---------------------------------------------------------------------------
// Thread
// ---------------------------------------------------------------------------

/// Un-initialise WinRT on the same thread that initialised it, on every exit
/// path of the thread.
struct RoApartment;
impl Drop for RoApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

fn run(tx: Sender<Msg>, cmd_rx: Receiver<Cmd>, stop: Arc<AtomicBool>) {
    // S_FALSE / RPC_E_CHANGED_MODE are both non-fatal; WinRT activation still
    // works, and a real failure surfaces at `RequestAsync` below.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let _apartment = RoApartment;

    let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| op.join())
    {
        Ok(m) => m,
        Err(e) => {
            let _ = tx.send(Msg::Error(format!("媒体会话不可用：{e}")));
            return;
        }
    };

    let dirty = Arc::new(AtomicBool::new(false));
    let mut session: Option<Subscribed> = None;
    // Cover art is re-read only when the track identity changes.
    let mut cover_key = String::new();
    let mut cover: Option<Arc<Cover>> = None;
    let mut next_poll = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match cmd_rx.recv_timeout(TICK) {
            Ok(cmd) => {
                if let Some(s) = &session {
                    run_cmd(&s.session, cmd);
                    dirty.store(true, Ordering::Relaxed);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        if !dirty.swap(false, Ordering::Relaxed) && next_poll.elapsed() < POLL {
            continue;
        }
        next_poll = Instant::now();

        // Any read failure counts as "no session": a closing player is not an
        // error worth putting on screen.
        let current = manager.GetCurrentSession().ok();
        session = reconcile(session, current, &dirty);
        let snap = session
            .as_ref()
            .and_then(|s| sample(&s.session, &mut cover_key, &mut cover));
        if tx.send(Msg::Now(snap)).is_err() {
            return;
        }
    }
}

fn run_cmd(session: &GlobalSystemMediaTransportControlsSession, cmd: Cmd) {
    // The results are advisory: a player may refuse the key, and there is
    // nothing to report that the next snapshot would not show.
    let _ = match cmd {
        Cmd::Prev => session.TrySkipPreviousAsync(),
        Cmd::Toggle => session.TryTogglePlayPauseAsync(),
        Cmd::Next => session.TrySkipNextAsync(),
    }
    .and_then(|op| op.join());
}

// ---------------------------------------------------------------------------
// Session subscriptions
// ---------------------------------------------------------------------------

struct Subscribed {
    session: GlobalSystemMediaTransportControlsSession,
    media: Option<i64>,
    playback: Option<i64>,
    timeline: Option<i64>,
}

impl Subscribed {
    fn new(
        session: GlobalSystemMediaTransportControlsSession,
        dirty: &Arc<AtomicBool>,
    ) -> Subscribed {
        let flag = Arc::clone(dirty);
        let media = session
            .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                flag.store(true, Ordering::Relaxed);
                Ok(())
            }))
            .ok();
        let flag = Arc::clone(dirty);
        let playback = session
            .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                flag.store(true, Ordering::Relaxed);
                Ok(())
            }))
            .ok();
        let flag = Arc::clone(dirty);
        let timeline = session
            .TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                flag.store(true, Ordering::Relaxed);
                Ok(())
            }))
            .ok();
        Subscribed {
            session,
            media,
            playback,
            timeline,
        }
    }

    fn unsubscribe(&self) {
        if let Some(t) = self.media {
            let _ = self.session.RemoveMediaPropertiesChanged(t);
        }
        if let Some(t) = self.playback {
            let _ = self.session.RemovePlaybackInfoChanged(t);
        }
        if let Some(t) = self.timeline {
            let _ = self.session.RemoveTimelinePropertiesChanged(t);
        }
    }
}

fn reconcile(
    old: Option<Subscribed>,
    current: Option<GlobalSystemMediaTransportControlsSession>,
    dirty: &Arc<AtomicBool>,
) -> Option<Subscribed> {
    let same = match (&old, &current) {
        (Some(o), Some(c)) => windows::core::Interface::as_raw(&o.session) == windows::core::Interface::as_raw(c),
        (None, None) => true,
        _ => false,
    };
    if same {
        return old;
    }
    if let Some(o) = old {
        o.unsubscribe();
    }
    current.map(|s| Subscribed::new(s, dirty))
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

fn sample(
    session: &GlobalSystemMediaTransportControlsSession,
    cover_key: &mut String,
    cover: &mut Option<Arc<Cover>>,
) -> Option<Snapshot> {
    let at = Instant::now();
    // A session that cannot answer a synchronous properties read is closing.
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
    let album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();

    let (playing, can_prev, can_toggle, can_next, rate) = match session.GetPlaybackInfo() {
        Ok(info) => {
            let playing = info
                .PlaybackStatus()
                .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
                .unwrap_or(false);
            let rate = info
                .PlaybackRate()
                .ok()
                .and_then(|r| r.Value().ok())
                .filter(|r| *r > 0.0 && *r <= 16.0)
                .unwrap_or(1.0);
            let (can_prev, can_toggle, can_next) = match info.Controls() {
                Ok(c) => (
                    c.IsPreviousEnabled().unwrap_or(false),
                    c.IsPlayPauseToggleEnabled().unwrap_or(true),
                    c.IsNextEnabled().unwrap_or(false),
                ),
                Err(_) => (false, true, false),
            };
            (playing, can_prev, can_toggle, can_next, rate)
        }
        Err(_) => (false, false, true, false, 1.0),
    };

    // TimeSpan is 100 ns ticks. A player may report a non-zero start offset
    // (audiobooks, some podcasts); both ends are relative to it.
    const TICKS: f64 = 10_000_000.0;
    let (position, duration) = match session.GetTimelineProperties() {
        Ok(t) => {
            let start = t.StartTime().map(|v| v.Duration).unwrap_or(0);
            let pos = t.Position().map(|v| v.Duration).unwrap_or(0);
            let end = t.EndTime().map(|v| v.Duration).unwrap_or(0);
            (
                (pos.saturating_sub(start).max(0) as f64 / TICKS).max(0.0),
                (end.saturating_sub(start).max(0) as f64 / TICKS).max(0.0),
            )
        }
        Err(_) => (0.0, 0.0),
    };

    let key = format!("{title}\u{1}{artist}\u{1}{album}");
    if *cover_key != key {
        *cover_key = key;
        *cover = read_cover(&props).map(Arc::new);
    }

    // The clone is per-sample and only copies the `Arc`.
    let cover = cover.clone();
    Some(Snapshot {
        title,
        artist,
        album,
        playing,
        can_prev,
        can_toggle,
        can_next,
        position,
        duration,
        rate,
        at,
        cover,
    })
}

fn read_cover(props: &GlobalSystemMediaTransportControlsSessionMediaProperties) -> Option<Cover> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > COVER_MAX_BYTES {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.join().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;

    let img = image::load_from_memory(&bytes).ok()?;
    let img = if img.width() > COVER_MAX || img.height() > COVER_MAX {
        img.resize(COVER_MAX, COVER_MAX, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let id = fnv1a(rgba.as_raw());
    Some(Cover {
        id,
        w,
        h,
        rgba: rgba.into_raw(),
    })
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(title: &str, position: f64, playing: bool, at: Instant) -> Snapshot {
        Snapshot {
            title: title.to_string(),
            artist: "artist".into(),
            album: "album".into(),
            playing,
            can_prev: false,
            can_toggle: true,
            can_next: false,
            position,
            duration: 300.0,
            rate: 1.0,
            at,
            cover: None,
        }
    }

    /// The bug this module exists to survive: a player that publishes its
    /// timeline once and never moves it. The clock must keep running instead
    /// of being pinned to the stale value every poll.
    #[test]
    fn frozen_timeline_keeps_the_clock_running() {
        let mut np = NowPlaying::new();
        np.apply(Some(snap("t", 2.3, true, Instant::now() - Duration::from_secs(6))));
        np.apply(Some(snap("t", 2.3, true, Instant::now() - Duration::from_secs(1))));
        let p = np.position();
        assert!((8.0..=9.0).contains(&p), "position {p} should be ~8.3");
    }

    #[test]
    fn a_seek_is_adopted() {
        let mut np = NowPlaying::new();
        np.apply(Some(snap("t", 10.0, true, Instant::now())));
        np.apply(Some(snap("t", 120.0, true, Instant::now())));
        let p = np.position();
        assert!((119.0..=121.0).contains(&p), "position {p} should be ~120");
    }

    #[test]
    fn new_track_rebases() {
        let mut np = NowPlaying::new();
        np.apply(Some(snap("one", 200.0, true, Instant::now())));
        np.apply(Some(snap("two", 0.0, true, Instant::now())));
        let p = np.position();
        assert!(p < 1.0, "position {p} should restart at 0");
    }

    /// Pausing must hold the clock where it was drawn, not snap it back to a
    /// timeline that stopped updating seconds ago.
    #[test]
    fn pause_holds_the_drawn_position() {
        let mut np = NowPlaying::new();
        np.apply(Some(snap("t", 30.0, true, Instant::now() - Duration::from_secs(5))));
        np.apply(Some(snap("t", 30.0, false, Instant::now())));
        let p = np.position();
        assert!((34.5..=35.5).contains(&p), "position {p} should hold at ~35");
        assert!(!np.snap().unwrap().playing);
    }

    /// Playback speed changes invalidate the extrapolation, so the reported
    /// position wins once.
    #[test]
    fn rate_change_rebases() {
        let mut np = NowPlaying::new();
        np.apply(Some(snap("t", 60.0, true, Instant::now())));
        let mut fast = snap("t", 60.0, true, Instant::now());
        fast.rate = 2.0;
        np.apply(Some(fast));
        assert!((np.rate - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_session_clears_the_clock() {
        let mut np = NowPlaying::new();
        np.apply(Some(snap("t", 42.0, true, Instant::now())));
        np.apply(None);
        assert!(np.position() == 0.0);
        assert!(np.snap().is_none());
    }
}
