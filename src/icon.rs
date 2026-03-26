use eframe::egui;
use std::sync::Arc;

const ICON_CONTENT_SCALE: f32 = 0.82;
const ICON_CORNER_RADIUS_RATIO: f32 = 0.22;

pub fn load_app_icon() -> Option<Arc<egui::IconData>> {
    let icon_bytes = include_bytes!("../icon.png");
    let mut icon = eframe::icon_data::from_png_bytes(icon_bytes).ok()?;
    inset_icon_content(&mut icon, ICON_CONTENT_SCALE);
    round_scaled_content_corners(&mut icon, ICON_CONTENT_SCALE, ICON_CORNER_RADIUS_RATIO);
    Some(Arc::new(icon))
}

fn inset_icon_content(icon: &mut egui::IconData, content_scale: f32) {
    let width = icon.width as usize;
    let height = icon.height as usize;
    if width == 0 || height == 0 {
        return;
    }

    let scale = content_scale.clamp(0.1, 1.0);
    if (scale - 1.0).abs() < f32::EPSILON {
        return;
    }

    let src = icon.rgba.clone();
    let mut dst = vec![0u8; src.len()];
    let src_w = width as f32;
    let src_h = height as f32;
    let inv_scale = 1.0 / scale;

    for y in 0..height {
        for x in 0..width {
            let norm_x = (x as f32 + 0.5) / src_w;
            let norm_y = (y as f32 + 0.5) / src_h;

            let src_norm_x = ((norm_x - 0.5) * inv_scale) + 0.5;
            let src_norm_y = ((norm_y - 0.5) * inv_scale) + 0.5;

            if !(0.0..=1.0).contains(&src_norm_x) || !(0.0..=1.0).contains(&src_norm_y) {
                continue;
            }

            let sx = ((src_norm_x * src_w) - 0.5).round().clamp(0.0, src_w - 1.0) as usize;
            let sy = ((src_norm_y * src_h) - 0.5).round().clamp(0.0, src_h - 1.0) as usize;

            let src_idx = (sy * width + sx) * 4;
            let dst_idx = (y * width + x) * 4;
            dst[dst_idx..dst_idx + 4].copy_from_slice(&src[src_idx..src_idx + 4]);
        }
    }

    icon.rgba = dst;
}

fn round_scaled_content_corners(icon: &mut egui::IconData, content_scale: f32, radius_ratio: f32) {
    let width = icon.width as usize;
    let height = icon.height as usize;
    if width == 0 || height == 0 {
        return;
    }

    let scale = content_scale.clamp(0.1, 1.0);
    let content_w = width as f32 * scale;
    let content_h = height as f32 * scale;
    let left = (width as f32 - content_w) * 0.5;
    let top = (height as f32 - content_h) * 0.5;
    let right = left + content_w;
    let bottom = top + content_h;

    let radius = (content_w.min(content_h) * radius_ratio.clamp(0.01, 0.49)).max(1.0);
    let feather = 1.5_f32;

    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            if px < left || px > right || py < top || py > bottom {
                continue;
            }

            let dx = if px < left + radius {
                (left + radius) - px
            } else if px > right - radius {
                px - (right - radius)
            } else {
                0.0
            };

            let dy = if py < top + radius {
                (top + radius) - py
            } else if py > bottom - radius {
                py - (bottom - radius)
            } else {
                0.0
            };

            if dx == 0.0 || dy == 0.0 {
                continue;
            }

            let dist = (dx * dx + dy * dy).sqrt();
            let alpha_mul = ((radius - dist + feather) / feather).clamp(0.0, 1.0);
            let idx = (y * width + x) * 4 + 3;
            let original_alpha = icon.rgba[idx] as f32 / 255.0;
            icon.rgba[idx] = (original_alpha * alpha_mul * 255.0).round() as u8;
        }
    }
}
