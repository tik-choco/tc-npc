//! Screen / window capture (ports Go `agent-vision/internal/capture`).
//!
//! Unlike the Go original (which drives the Win32 API directly for window
//! capture), this uses the cross-platform `xcap` crate for both monitor and
//! window enumeration/capture.

use std::path::Path;

use base64::Engine;
use image::{imageops::FilterType, DynamicImage, RgbaImage};
use npc_core::config::VisionConfig;

/// Longest edge, in pixels, images are downscaled to before being sent to
/// the VLM — keeps multimodal payloads reasonably sized.
const MAX_EDGE: u32 = 1280;

/// Capture a screenshot per `config`, honoring `capture_mode` /
/// `target_window_title`, downscaling oversized images, and optionally
/// writing a debug PNG under `data_dir` (never the process cwd).
pub fn capture(config: &VisionConfig, data_dir: &Path) -> anyhow::Result<RgbaImage> {
    let use_window = config.capture_mode == "active" && !config.target_window_title.is_empty();

    let img = if use_window {
        match capture_window(&config.target_window_title) {
            Ok(img) => img,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    target = %config.target_window_title,
                    "vision: window capture failed, falling back to entire screen"
                );
                capture_entire_screen()?
            }
        }
    } else {
        capture_entire_screen()?
    };

    let img = downscale(img);

    if config.debug_save {
        let path = data_dir.join("debug_capture.png");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match img.save(&path) {
            Ok(()) => tracing::debug!(path = %path.display(), "vision: saved debug capture"),
            Err(err) => {
                tracing::warn!(error = %err, path = %path.display(), "vision: failed to save debug capture")
            }
        }
    }

    Ok(img)
}

/// Encode an image as a PNG `data:` URI, e.g. `data:image/png;base64,...`.
pub fn encode_data_url(img: &RgbaImage) -> anyhow::Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    DynamicImage::ImageRgba8(img.clone()).write_to(&mut cursor, image::ImageFormat::Png)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(format!("data:image/png;base64,{encoded}"))
}

fn capture_entire_screen() -> anyhow::Result<RgbaImage> {
    let monitors = xcap::Monitor::all().map_err(|e| anyhow::anyhow!("{e}"))?;
    if monitors.is_empty() {
        anyhow::bail!("no active displays found");
    }
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .unwrap_or(&monitors[0]);
    monitor
        .capture_image()
        .map_err(|e| anyhow::anyhow!("failed to capture monitor: {e}"))
}

/// Find a visible window matching `target` and capture it.
///
/// If `target` ends with `.exe`, it is matched (case-insensitively, exact)
/// against the window's app/process name; otherwise `target` is matched as a
/// case-insensitive substring of the window title. Note: xcap's `app_name()`
/// returns a friendly product/file-description name on Windows rather than
/// the literal executable filename the Go original compared against via
/// `Process32Next`'s `szExeFile`, so `.exe`-style matches are best-effort —
/// this also tries a substring match against the (extension-stripped)
/// target as a fallback.
fn capture_window(target: &str) -> anyhow::Result<RgbaImage> {
    let target_lower = target.to_lowercase();
    let is_exe = target_lower.ends_with(".exe");
    let stem = target_lower.strip_suffix(".exe").unwrap_or(&target_lower);

    let windows = xcap::Window::all().map_err(|e| anyhow::anyhow!("{e}"))?;

    let window = windows.into_iter().find(|w| {
        if w.is_minimized().unwrap_or(false) {
            return false;
        }
        if is_exe {
            let app_name = w.app_name().unwrap_or_default().to_lowercase();
            app_name == target_lower || app_name == stem || app_name.contains(stem)
        } else {
            w.title()
                .map(|t| t.to_lowercase().contains(&target_lower))
                .unwrap_or(false)
        }
    });

    let window =
        window.ok_or_else(|| anyhow::anyhow!("visible window not found for target '{target}'"))?;

    window
        .capture_image()
        .map_err(|e| anyhow::anyhow!("failed to capture window: {e}"))
}

/// Downscale `img` so its longest edge is at most [`MAX_EDGE`] pixels,
/// preserving aspect ratio. A no-op if the image is already small enough.
fn downscale(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let longest = w.max(h);
    if longest <= MAX_EDGE || longest == 0 {
        return img;
    }
    let scale = MAX_EDGE as f32 / longest as f32;
    let new_w = ((w as f32) * scale).round().max(1.0) as u32;
    let new_h = ((h as f32) * scale).round().max(1.0) as u32;
    DynamicImage::ImageRgba8(img)
        .resize(new_w, new_h, FilterType::Lanczos3)
        .to_rgba8()
}
