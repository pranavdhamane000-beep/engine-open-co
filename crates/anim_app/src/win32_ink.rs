//! Win32 pointer history, pen metadata and capture (spec 19.3 win32_ink.rs,
//! 19.6). Windows-only; supplies pen metadata (pressure, tilt, eraser,
//! primary/contact flags) and coalesced history to the editor's ink engine.
//!
//! Event capture inside a winit window requires a WM_POINTER-integrated
//! window subclass (landed with the WP11 app integration). This module is
//! the conversion + probe layer: given a real pen info frame, it produces
//! editor `PointerEvent`s; the integration decides whether native
//! subclassing is active (spec 19.11: a subclass failure disables Ink with
//! an explicit diagnostic while preserving mouse editing).

use anim_editor::{PointerEvent, PointerEventKind, RawPoint};

/// Device pressure range (Windows pen spec): 0..=1024 normalizes to [0,1].
pub const PRESSURE_DEVICE_MAX: u32 = 1_024;

/// Win32 `PEN_FLAG_ERASER` value. The windows crate projection exposes
/// `penFlags` as a plain u32 but omits the PEN_FLAG_* constants, so the
/// documented bit values are carried here and versioned with the payload.
pub const PEN_FLAG_ERASER: u32 = 0x0000_0001;
/// Win32 `PEN_FLAG_INVERT`.
pub const PEN_FLAG_INVERT: u32 = 0x0000_0002;
/// Win32 `PEN_FLAG_BARREL`.
pub const PEN_FLAG_BARREL: u32 = 0x0000_0004;

/// Query the pointer type of an active pointer id (real Win32 call; the
/// integration filters pen-only pointers before strokes start). Returns the
/// `POINTER_INPUT_TYPE` tag value on success — PT_PEN = 2.
#[cfg(windows)]
pub fn pointer_type(pointer_id: u32) -> Option<u32> {
    use windows::Win32::UI::Input::Pointer::GetPointerType;
    use windows::Win32::UI::WindowsAndMessaging::POINTER_INPUT_TYPE;

    let mut pointer_type = POINTER_INPUT_TYPE(0i32);
    // SAFETY: pointer_id came from a live WM_POINTER frame in this session;
    // the out-param is a local initialized value, never dereferenced by the
    // callee without a successful return.
    let result = unsafe { GetPointerType(pointer_id, &mut pointer_type) };
    result.ok().map(|_| pointer_type.0 as u32)
}

/// Convert one Win32 pen info frame into an editor pointer event.
/// `history` must be ordered oldest to newest (spec 19.6, 19.12).
#[cfg(windows)]
pub fn pen_info_to_event(
    kind: PointerEventKind,
    info: &windows::Win32::UI::Input::Pointer::POINTER_PEN_INFO,
    history: Vec<RawPoint>,
) -> PointerEvent {
    use windows::Win32::UI::Input::Pointer::{
        POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_PRIMARY,
    };

    let pi = &info.pointerInfo;
    // Pressure normalizes 0..=1024 -> [0,1]; the engine clamps again
    // defensively (spec 19.7).
    let pressure = (info.pressure as f32 / PRESSURE_DEVICE_MAX as f32).clamp(0.0, 1.0);
    // dwTime is milliseconds since boot; the editor needs microseconds.
    // Overflow is technically possible after >584k years; u64 saturates.
    let timestamp_us = (pi.dwTime as u64).saturating_mul(1_000);
    let contact = pi.pointerFlags.contains(POINTER_FLAG_INCONTACT | POINTER_FLAG_INRANGE);
    // Hover events (in-range without contact) carry pressure as metadata
    // only; the engine treats them as non-mutating (spec 19.7).
    let hover =
        kind == PointerEventKind::Hover || (!contact && kind == PointerEventKind::Update);
    PointerEvent {
        kind: if hover { PointerEventKind::Hover } else { kind },
        pointer_id: pi.pointerId,
        is_primary: pi.pointerFlags.contains(POINTER_FLAG_PRIMARY),
        // Pen flags decide the tool: eraser bit (or the barrel mask renamed)
        // selects the eraser path (spec 19.7).
        is_eraser: (info.penFlags & PEN_FLAG_ERASER) != 0
            || (info.penFlags & PEN_FLAG_BARREL) != 0,
        // Win32 digitizer input ALSO synthesizes legacy mouse messages;
        // those are dropped at the winit layer, never reach here.
        is_mouse_synthesized: false,
        // POINT coordinates are client-space pixels for WM_POINTER input.
        client_x: pi.ptPixelLocation.x as f32,
        client_y: pi.ptPixelLocation.y as f32,
        pressure,
        // Pen tilts are i32 in the projection (i16 in the SDK docs as
        // INT16); narrowing never truncates within [-90, 90].
        tilt_x: info.tiltX as i16,
        tilt_y: info.tiltY as i16,
        timestamp_us,
        history,
    }
}

/// Normalize one raw device sample into an editor history point.
#[cfg(windows)]
pub fn pen_sample_to_raw(
    info: &windows::Win32::UI::Input::Pointer::POINTER_PEN_INFO,
) -> RawPoint {
    let pi = &info.pointerInfo;
    RawPoint {
        client_x: pi.ptPixelLocation.x as f32,
        client_y: pi.ptPixelLocation.y as f32,
        pressure: (info.pressure as f32 / PRESSURE_DEVICE_MAX as f32).clamp(0.0, 1.0),
        tilt_x: info.tiltX as i16,
        tilt_y: info.tiltY as i16,
        timestamp_us: (pi.dwTime as u64).saturating_mul(1_000),
    }
}
