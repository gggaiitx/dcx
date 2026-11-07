//! Background sampler for CPU, memory, disk and network.
//!
//! One thread, one period, one shared snapshot. The UI never calls a sampling
//! API itself: `GetIfTable2` allocates and walks the interface table, which is
//! far too heavy to run inside a frame, and `GetSystemTimes` needs a fixed
//! interval to produce a percentage.
//!
//! Rate maths lives here rather than in the widget: bytes per second must be
//! measured over the *actual* elapsed time, not the configured period — a
//! sample that was late by 400 ms would otherwise inflate the readout by 40 %.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    /// 0–100, smoothed.
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub mem_percent: f32,
    pub disk_used: u64,
    pub disk_total: u64,
    pub net_down_bps: f64,
    pub net_up_bps: f64,
}

pub struct Sampler {
    pub snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    interval_ms: Arc<Mutex<u64>>,
    /// Set when a sampling call failed; the panel shows it instead of a
    /// permanently frozen number.
    pub error: Arc<Mutex<Option<String>>>,
}

impl Sampler {
    pub fn start(interval_secs: u64) -> Sampler {
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let interval_ms = Arc::new(Mutex::new(interval_secs.max(1) * 1000));
        let error = Arc::new(Mutex::new(None));

        let snap = Arc::clone(&snapshot);
        let stop2 = Arc::clone(&stop);
        let period = Arc::clone(&interval_ms);
        let err2 = Arc::clone(&error);

        std::thread::Builder::new()
            .name("dcx-island-metrics".to_string())
            .spawn(move || {
                let mut prev_cpu = crate::sys::cpu_sample();
                let mut prev_net = crate::sys::net_octets();
                let mut prev_at = Instant::now();
                let mut smoothed: Option<Snapshot> = None;

                while !stop2.load(Ordering::SeqCst) {
                    let target = Duration::from_millis(*period.lock().unwrap_or_else(|e| e.into_inner()));
                    // Sleep in slices so a stop request is honoured promptly.
                    let mut slept = Duration::ZERO;
                    while slept < target && !stop2.load(Ordering::SeqCst) {
                        let step = Duration::from_millis(100).min(target - slept);
                        std::thread::sleep(step);
                        slept += step;
                    }
                    if stop2.load(Ordering::SeqCst) {
                        return;
                    }

                    let now = Instant::now();
                    let elapsed = now.duration_since(prev_at).as_secs_f64().max(0.001);
                    prev_at = now;

                    let mut s = Snapshot::default();

                    let cpu_now = crate::sys::cpu_sample();
                    s.cpu = match (&prev_cpu, &cpu_now) {
                        (Some(a), Some(b)) => crate::sys::cpu_percent(a, b),
                        _ => 0.0,
                    };
                    if cpu_now.is_some() {
                        prev_cpu = cpu_now;
                    }

                    match crate::sys::memory() {
                        Some((total, avail, load)) => {
                            s.mem_total = total;
                            s.mem_used = total.saturating_sub(avail);
                            s.mem_percent = load as f32;
                            // A one-off failure must not pin the panel's error
                            // line for the rest of the session.
                            let mut slot = err2.lock().unwrap_or_else(|e| e.into_inner());
                            if slot.is_some() {
                                *slot = None;
                            }
                        }
                        None => *err2.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some("无法读取内存状态".to_string()),
                    }

                    if let Some(drive) = system_drive() {
                        if let Some((total, free)) = crate::sys::disk(&drive) {
                            s.disk_total = total;
                            s.disk_used = total.saturating_sub(free);
                        }
                    }

                    let net_now = crate::sys::net_octets();
                    match (&prev_net, &net_now) {
                        (Some((pin, pout)), Some((nin, nout))) => {
                            s.net_down_bps =
                                nin.saturating_sub(*pin) as f64 / elapsed;
                            s.net_up_bps = nout.saturating_sub(*pout) as f64 / elapsed;
                        }
                        _ => {}
                    }
                    if net_now.is_some() {
                        prev_net = net_now;
                    }

                    // 400 ms ease towards the new sample: the raw 1 s value
                    // jumps between 3 % and 40 % on an idle machine and reads
                    // as noise rather than a measurement.
                    let out = match smoothed {
                        None => s,
                        Some(prev) => Snapshot {
                            cpu: ease(prev.cpu, s.cpu),
                            mem_used: s.mem_used,
                            mem_total: s.mem_total,
                            mem_percent: ease(prev.mem_percent, s.mem_percent),
                            disk_used: s.disk_used,
                            disk_total: s.disk_total,
                            net_down_bps: ease64(prev.net_down_bps, s.net_down_bps),
                            net_up_bps: ease64(prev.net_up_bps, s.net_up_bps),
                        },
                    };
                    smoothed = Some(out);
                    *snap.lock().unwrap_or_else(|e| e.into_inner()) = out;
                }
            })
            .ok();

        Sampler {
            snapshot,
            stop,
            interval_ms,
            error,
        }
    }

    pub fn read(&self) -> Snapshot {
        *self.snapshot.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_interval(&self, secs: u64) {
        *self.interval_ms.lock().unwrap_or_else(|e| e.into_inner()) = secs.max(1) * 1000;
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 0.4 of the distance to the new value: close enough to track a real spike,
/// far enough to hide sampling jitter.
fn ease(prev: f32, next: f32) -> f32 {
    prev + (next - prev) * 0.4
}

fn ease64(prev: f64, next: f64) -> f64 {
    prev + (next - prev) * 0.4
}

/// `%SystemDrive%` — asking the API for every volume would add a second of I/O
/// per sample and the widget shows one number anyway.
pub fn system_drive() -> Option<String> {
    std::env::var("SystemDrive")
        .ok()
        .map(|d| format!("{d}\\"))
        .or_else(|| Some("C:\\".to_string()))
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Network rates get their own scale: `1.2 MB/s` reads better than
/// `1.2 MB` and the unit suffix is per second.
pub fn human_rate(bps: f64) -> String {
    if bps < 1024.0 {
        format!("{bps:.0} B/s")
    } else if bps < 1024.0 * 1024.0 {
        format!("{:.0} KB/s", bps / 1024.0)
    } else {
        format!("{:.1} MB/s", bps / (1024.0 * 1024.0))
    }
}

/// Colour band for a percentage, matching the reference behaviour: normal,
/// warning at 75 %, critical at 90 %.
pub fn level(percent: f32) -> u8 {
    if percent >= 90.0 {
        2
    } else if percent >= 75.0 {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1024 * 1024 * 3), "3.0 MB");
        assert_eq!(human_bytes(1024u64.pow(3) * 2), "2.0 GB");
    }

    #[test]
    fn rates_use_per_second_units() {
        assert_eq!(human_rate(500.0), "500 B/s");
        assert_eq!(human_rate(2048.0), "2 KB/s");
        assert_eq!(human_rate(2.0 * 1024.0 * 1024.0), "2.0 MB/s");
    }

    #[test]
    fn levels_match_the_thresholds() {
        assert_eq!(level(10.0), 0);
        assert_eq!(level(75.0), 1);
        assert_eq!(level(90.0), 2);
    }

    #[test]
    fn a_sampler_produces_a_plausible_snapshot() {
        let s = Sampler::start(1);
        std::thread::sleep(Duration::from_millis(1600));
        let snap = s.read();
        assert!(snap.mem_total > 0, "memory total must be readable");
        assert!(snap.mem_percent <= 100.0);
        assert!(snap.cpu >= 0.0 && snap.cpu <= 100.0);
    }
}
