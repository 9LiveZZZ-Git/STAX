use egui::{Painter, Rect, Stroke, StrokeKind, pos2};
use crate::shell;

/// Draw a waveform display in `rect`.
/// `samples`: PCM f32 slice (can be empty — draws flat line).
/// Design: SURFACE background, RULE_2 center line, WARM waveform line (1px), thin RULE border.
pub fn draw_scope(painter: &Painter, rect: Rect, samples: &[f32]) {
    // Background
    painter.rect_filled(rect, 0.0, shell::SURFACE);

    // Center line
    let cy = rect.center().y;
    painter.line_segment(
        [pos2(rect.min.x, cy), pos2(rect.max.x, cy)],
        Stroke::new(0.5, shell::RULE_2),
    );

    // Waveform polyline
    if !samples.is_empty() {
        let max_abs = samples
            .iter()
            .map(|s| s.abs())
            .fold(0.0_f32, f32::max)
            .max(1e-6);

        let w = rect.width();
        let h = rect.height();
        let half_h = h * 0.5;
        let n = samples.len() as f32;

        let points: Vec<egui::Pos2> = samples
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let x = rect.min.x + (i as f32 / (n - 1.0).max(1.0)) * w;
                let y = cy - (s / max_abs) * half_h;
                pos2(x, y)
            })
            .collect();

        painter.add(egui::Shape::line(points, Stroke::new(1.0, shell::WARM)));
    }

    // Border
    painter.rect_stroke(rect, 0.0, Stroke::new(0.5, shell::RULE), StrokeKind::Outside);
}

/// Draw a level meter (horizontal bar) in `rect`.
/// `level`: 0.0..1.0 RMS level.
/// Design: SURFACE bg, COOL fill proportional to level, RULE border.
pub fn draw_meter(painter: &Painter, rect: Rect, level: f32) {
    // Background
    painter.rect_filled(rect, 0.0, shell::SURFACE);

    // Fill bar
    let fill_w = (level.clamp(0.0, 1.0) * rect.width()).max(0.0);
    if fill_w > 0.0 {
        let fill_rect = Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height()));
        painter.rect_filled(fill_rect, 0.0, shell::COOL);
    }

    // Border
    painter.rect_stroke(rect, 0.0, Stroke::new(0.5, shell::RULE), StrokeKind::Outside);
}

/// Compute RMS of a sample slice. Returns 0.0 for an empty slice.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}
