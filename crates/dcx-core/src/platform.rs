//! Event loop, WinEvent hook installation, and frame pacing.

use crate::effect::{EffectCtx, EffectEntry};
use crate::event::WindowEvent;
use dcx_win32::*;
use std::sync::Mutex;

/// Events are queued from the WinEvent callback and drained on the frame tick.
/// Keeping the callback free of effect logic avoids re-entrancy and deadlocks.
static QUEUE: Mutex<Vec<WindowEvent>> = Mutex::new(Vec::new());

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: DWORD,
    hwnd: HWND,
    id_object: LONG,
    _id_child: LONG,
    _thread: DWORD,
    _time: DWORD,
) {
    // idObject 0 == OBJID_WINDOW; ignore child-level noise entirely.
    if id_object != 0 || hwnd == 0 {
        return;
    }
    let ev = match event {
        0x0003 => WindowEvent::Foreground(hwnd), // EVENT_SYSTEM_FOREGROUND
        _ => return,
    };
    if let Ok(mut q) = QUEUE.lock() {
        // Hard cap so a storm of events can't grow without bound.
        if q.len() < 64 {
            q.push(ev);
        }
    }
}

pub fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: UINT) -> UINT;
    fn timeEndPeriod(period: UINT) -> UINT;
}

pub struct Platform {
    effects: Vec<EffectEntry>,
    start_ms: u64,
    last_ms: u64,
    timer_ms: u32,
    high_res_timer: bool,
}

impl Platform {
    pub fn new(timer_ms: u32) -> Self {
        let now = now_ms();
        Self {
            effects: Vec::new(),
            start_ms: now,
            last_ms: now,
            timer_ms: timer_ms.max(1),
            high_res_timer: true,
        }
    }

    /// Trade a little idle power for smooth animation. Set false for battery-first.
    pub fn with_high_res_timer(&mut self, on: bool) -> &mut Self {
        self.high_res_timer = on;
        self
    }
    pub fn register(&mut self, effect: Box<dyn crate::effect::Effect>) -> &mut Self {
        self.effects.push(EffectEntry::new(effect));
        self
    }

    fn tick(&mut self) {
        let now = now_ms();
        let dt = (now.saturating_sub(self.last_ms)) as f32;
        self.last_ms = now;
        let ctx = EffectCtx {
            now_ms: now.saturating_sub(self.start_ms),
            dt_ms: dt.clamp(0.5, 100.0),
        };

        let events: Vec<WindowEvent> = match QUEUE.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        };

        for ev in events {
            for entry in self.effects.iter_mut() {
                if !entry.enabled {
                    continue;
                }
                if entry.effect.on_event(ev, &ctx) {
                    break;
                }
            }
        }

        for entry in self.effects.iter_mut() {
            if entry.enabled {
                entry.effect.on_tick(&ctx);
            }
        }
    }

    /// Blocking message loop. Returns when the thread receives WM_QUIT.
    pub fn run(&mut self) -> i32 {
        self.run_inner(None)
    }

    /// Same as [`run`], but self-terminates after `ms`. Used by smoke tests.
    pub fn run_for(&mut self, ms: u64) -> i32 {
        self.run_inner(Some(ms))
    }

    fn run_inner(&mut self, limit_ms: Option<u64>) -> i32 {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            if self.high_res_timer {
                timeBeginPeriod(1);
            }

            let fg_hook = SetWinEventHook(
                0x0003,
                0x0003,
                0,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );

            // Keep the id the system assigned: `KillTimer(0, 0)` in the teardown
            // below addressed id 0, which does not exist, so the frame timer
            // survived every `run()`.
            let timer_id = SetTimer(0, 0, self.timer_ms, 0);

            if let Some(ms) = limit_ms {
                let tid = GetCurrentThreadId();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                    PostThreadMessageW(tid, WM_QUIT, 0, 0);
                });
            }

            crate::dcx_log!("frame loop started, {} effect(s)", self.effects.len());

            let mut msg = MSG::default();
            loop {
                let r = GetMessageW(&mut msg, 0, 0, 0);
                if r == -1 || r == 0 {
                    break;
                }
                if msg.message == WM_TIMER {
                    self.tick();
                    continue;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            if timer_id != 0 {
                KillTimer(0, timer_id);
            }
            if fg_hook != 0 {
                UnhookWinEvent(fg_hook);
            }
            for entry in self.effects.iter_mut() {
                entry.effect.shutdown();
            }
            if self.high_res_timer {
                timeEndPeriod(1);
            }
        }
        0
    }
}
