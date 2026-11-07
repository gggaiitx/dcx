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

#[derive(Clone, Debug, Default)]
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
    pub cover: Option<Cover>,
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

pub struct NowPlaying {
    snap: Option<Snapshot>,
    cover_tex: Option<(u64, egui::TextureHandle)>,
    /// Local clock for `position`; `Instant` has no `Default`, so the struct
    /// has no derived one either.
    at: Instant,
}

impl NowPlaying {
    pub fn new() -> Self {
        NowPlaying {
            snap: None,
            cover_tex: None,
            at: Instant::now(),
        }
    }

    pub fn set(&mut self, ctx: &egui::Context, snap: Option<Snapshot>) {
        if let Some(c) = snap.as_ref().and_then(|s| s.cover.as_ref()) {
            let stale = self.cover_tex.as_ref().map(|(id, _)| *id != c.id).unwrap_or(true);
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
        self.snap = snap;
        self.at = Instant::now();
    }

    pub fn snap(&self) -> Option<&Snapshot> {
        self.snap.as_ref()
    }

    pub fn cover(&self) -> Option<&egui::TextureHandle> {
        self.cover_tex.as_ref().map(|(_, t)| t)
    }

    /// The position to draw: the sampled value plus the local clock since the
    /// sample. WinRT is asked once per second, not once per frame.
    pub fn position(&self) -> f64 {
        let Some(s) = &self.snap else { return 0.0 };
        let mut pos = s.position
            + if s.playing {
                self.at.elapsed().as_secs_f64() * s.rate
            } else {
                0.0
            };
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
    let mut cover: Option<Cover> = None;
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
    cover: &mut Option<Cover>,
) -> Option<Snapshot> {
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

    // TimeSpan is 100 ns ticks.
    const TICKS: f64 = 10_000_000.0;
    let (position, duration) = match session.GetTimelineProperties() {
        Ok(t) => {
            let start = t.StartTime().map(|v| v.Duration).unwrap_or(0);
            let pos = t.Position().map(|v| v.Duration).unwrap_or(0);
            let end = t.EndTime().map(|v| v.Duration).unwrap_or(0);
            (
                ((pos - start).max(0) as f64 / TICKS).max(0.0),
                ((end - start).max(0) as f64 / TICKS).max(0.0),
            )
        }
        Err(_) => (0.0, 0.0),
    };

    let key = format!("{title}\u{1}{artist}\u{1}{album}");
    if *cover_key != key {
        *cover_key = key;
        *cover = read_cover(&props);
    }

    // The clone is per-sample; the RGBA buffer itself is shared until the
    // art changes.
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
