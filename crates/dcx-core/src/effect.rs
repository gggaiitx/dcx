//! The extension point of the platform.
//!
//! Every desktop effect is a type implementing [`Effect`]. The host owns a
//! `Vec<Box<dyn Effect>>` and feeds it window events plus a ~8 ms tick. An
//! effect that returns `true` from [`Effect::on_event`] claims the event and
//! stops propagation, so mutually exclusive effects compose predictably.

use crate::event::WindowEvent;

#[derive(Debug, Clone, Copy)]
pub struct EffectCtx {
    /// Monotonic milliseconds since host start.
    pub now_ms: u64,
    /// Milliseconds since the previous tick, clamped to a sane range.
    pub dt_ms: f32,
}

pub trait Effect {
    /// Stable identifier, also used as the config key.
    fn name(&self) -> &'static str;

    /// Handle one window event. Return `true` to stop propagation.
    fn on_event(&mut self, ev: WindowEvent, ctx: &EffectCtx) -> bool;

    /// Called every frame. Animation stepping lives here.
    fn on_tick(&mut self, _ctx: &EffectCtx) {}

    /// Release GDI/DWM resources. Called on shutdown.
    fn shutdown(&mut self) {}
}

/// Thin wrapper so the host can report per-effect stats later.
pub struct EffectEntry {
    pub effect: Box<dyn Effect>,
    pub enabled: bool,
}

impl EffectEntry {
    pub fn new(effect: Box<dyn Effect>) -> Self {
        Self {
            effect,
            enabled: true,
        }
    }
}
