//! anim_app — application composition and orderly shutdown (spec 19.3 main.rs).
//!
//! winit owns the event loop; the router maps events to actions; the editor
//! session owns play state, ink and quality. Per decision D-11: NO automatic
//! GPU selection happens here. At startup the engine prints a plain reminder
//! listing the GPUs it can see by name and tells the user to make sure the
//! discrete GPU is the one rendering. The enumeration is display-only: it
//! never requests a device and never reorders or picks by device type.

pub mod settings;
pub mod win32_ink;
pub mod window;

use anim_editor::{EditorSession, Tool};
use window::{Action, AppEvent, WindowRouter};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// Display-only adapter listing for the startup reminder. Returns the
/// human-readable message; the caller prints it.
fn gpu_startup_reminder() -> Vec<String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapters = instance.enumerate_adapters(wgpu::Backends::all());

    if adapters.is_empty() {
        return vec!["GPU startup notice: no graphics adapters enumerated.".into()];
    }

    let mut names: Vec<String> = adapters
        .iter()
        .map(|a| {
            let info = a.get_info();
            format!("{} ({:?}, {})", info.name, info.device_type, info.backend)
        })
        .collect();
    names.sort(); // display order only — never used to pick anything

    let mut lines =
        vec!["=== GPU reminder (no automatic selection, decision D-11) ===".into()];
    for (i, n) in names.iter().enumerate() {
        lines.push(format!("  [{}] {}", i + 1, n));
    }
    lines.push(
        "REMINDER: make sure the DISCRETE GPU (e.g. your NVIDIA GeForce RTX) is the one rendering:".into(),
    );
    lines.push("  - Windows: Settings > Display > Graphics > set this app / global preference to the dGPU".into());
    lines.push(
        "  - NVIDIA Control Panel: Manage 3D Settings > Preferred graphics processor"
            .into(),
    );
    lines.push(
        "The engine will simply use whatever adapter is active; the choice stays yours."
            .into(),
    );
    lines
}

/// The composed application (spec 19.5 step 10: orderly shutdown from the
/// editor session, transport paused, THEN the window closes).
struct App {
    router: WindowRouter,
    session: Option<EditorSession>,
    window: Option<Window>,
}

impl App {
    fn new() -> Self {
        let doc = anim_project::ProjectDocument::new(
            anim_core::FrameRate::new(24, 1).expect("24 fps is a valid rational"),
        );
        let session = EditorSession::new(
            anim_commands::CommandProcessor::new(doc),
            0,
            16_666.0,
            Tool::Disabled,
        );
        Self { router: WindowRouter::new(), session: Some(session), window: None }
    }

    /// Orderly shutdown (spec 19.5 step 10): pause transport, reset quality
    /// (device recovery path), publish, then close.
    fn shutdown_orderly(&mut self) {
        if let Some(session) = &self.session {
            session.transport_pause();
        }
        self.session = None;
        if let Some(w) = self.window.take() {
            w.set_visible(false);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title("The Arena — anim (WP10 scaffold)")
                .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
            match event_loop.create_window(attrs) {
                Ok(w) => {
                    w.request_redraw();
                    self.window = Some(w);
                }
                Err(e) => {
                    eprintln!("window creation failed, exiting orderly: {e}");
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let action = match event {
            WindowEvent::CloseRequested => self.router.route(AppEvent::CloseRequested),
            WindowEvent::RedrawRequested => self.router.route(AppEvent::RedrawRequested),
            WindowEvent::Resized(size) => self
                .router
                .route(AppEvent::Resized { width: size.width, height: size.height }),
            WindowEvent::Focused(focused) => {
                self.router.route(AppEvent::Focused { focused })
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Transport hotkeys operate via explicit session entry
                // points; no hidden edit paths here (spec 19.6).
                if let (ElementState::Pressed, Some(key)) =
                    (event.state, event.logical_key.to_text())
                {
                    if let Some(session) = &self.session {
                        if key == " " {
                            session.transport_play()
                        }
                    }
                }
                Action::None
            }
            _ => Action::None,
        };

        match action {
            Action::RequestRedraw => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            Action::Shutdown => {
                self.shutdown_orderly();
                event_loop.exit();
            }
            Action::HandleDeviceLoss => {
                if let Some(session) = &self.session {
                    session.reset_quality_full();
                }
            }
            Action::None => {}
        }
    }
}

fn main() {
    for line in gpu_startup_reminder() {
        println!("{line}");
    }

    let event_loop = EventLoop::builder().build().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new();

    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
    // Orderly shutdown completed inside App (spec 19.5 step 10).
}
