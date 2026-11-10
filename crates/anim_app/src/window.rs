//! winit routing and redraw scheduling (spec 19.3 window.rs, 19.10).
//!
//! The router owns the application's RESPONSE policy to window events in a
//! pure, testable form: winit delivers OS events, the app maps them here,
//! and the event loop executes the returned actions. No GPU selection
//! happens anywhere in this module (decision D-11).

/// Typed application events routed from winit.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    RedrawRequested,
    CloseRequested,
    Resized {
        width: u32,
        height: u32,
    },
    Focused {
        focused: bool,
    },
    /// Device recovery surface (renderer attach; WP11 completes wiring).
    DeviceRecovered,
    /// Application tick (animation advance, quality measurement).
    Tick {
        frame_time_us: f64,
    },
}

/// Actions the event loop must execute after routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Ask winit for a redraw.
    RequestRedraw,
    /// Leave the loop orderly (spec 19.5 step 10 shutdown).
    Shutdown,
    /// Drain device state (loss/recovery) before touching anything.
    HandleDeviceLoss,
    /// Nothing to do this iteration.
    None,
}

/// Redraw throughput policy: app MAY coalesce redraw requests; the engine
/// only ever draws on explicit requests, never unthrottled.
#[derive(Debug, Clone)]
pub struct WindowRouter {
    redraw_pending: bool,
    shutdown_requested: bool,
    /// Frames drawn since last quality measure (display only).
    frames_drawn: u64,
}

impl WindowRouter {
    pub fn new() -> Self {
        Self { redraw_pending: false, shutdown_requested: false, frames_drawn: 0 }
    }

    /// Pure event routing (spec 19.10: winit owns the event loop).
    pub fn route(&mut self, ev: AppEvent) -> Action {
        match ev {
            AppEvent::RedrawRequested => {
                self.redraw_pending = false;
                self.frames_drawn += 1;
                // Draw happened; schedule the next frame until shutdown.
                if self.shutdown_requested {
                    Action::None
                } else {
                    Action::RequestRedraw
                }
            }
            AppEvent::CloseRequested => {
                self.shutdown_requested = true;
                self.redraw_pending = false;
                Action::Shutdown
            }
            AppEvent::Resized { .. } => {
                // Resize invalidates quality samples (spec 19.7); the
                // session applies that at a higher layer; here a redraw
                // suffices so the surface is re-presented.
                Action::RequestRedraw
            }
            AppEvent::Focused { focused } => {
                if focused {
                    Action::RequestRedraw
                } else {
                    // Losing focus pauses interactive advance; isolated
                    // preview output stays available (spec 19.11).
                    Action::None
                }
            }
            AppEvent::DeviceRecovered => Action::HandleDeviceLoss,
            AppEvent::Tick { .. } => Action::RequestRedraw,
        }
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown_requested
    }

    pub fn frames_drawn(&self) -> u64 {
        self.frames_drawn
    }
}

impl Default for WindowRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_requests_shutdown_and_stops_redraws() {
        let mut r = WindowRouter::new();
        assert_eq!(r.route(AppEvent::RedrawRequested), Action::RequestRedraw);
        assert_eq!(r.route(AppEvent::CloseRequested), Action::Shutdown);
        assert!(r.is_shutdown());
        assert_eq!(
            r.route(AppEvent::RedrawRequested),
            Action::None,
            "shutdown accepted; loop must drain, not draw"
        );
    }

    #[test]
    fn tick_drives_redraw_and_resize_requests_one() {
        let mut r = WindowRouter::new();
        assert_eq!(
            r.route(AppEvent::Tick { frame_time_us: 16_666.0 }),
            Action::RequestRedraw
        );
        assert_eq!(
            r.route(AppEvent::Resized { width: 1280, height: 720 }),
            Action::RequestRedraw
        );
        assert_eq!(r.frames_drawn(), 0, "only a served redraw counts a frame");
        assert_eq!(r.route(AppEvent::RedrawRequested), Action::RequestRedraw);
        assert_eq!(r.frames_drawn(), 1, "served redraw bumps the counter");
    }
}
