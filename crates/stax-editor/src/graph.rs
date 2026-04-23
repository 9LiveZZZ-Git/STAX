use std::collections::HashMap;
use egui::{Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};
use stax_graph::{Graph, Node, NodeId, PortKind};
use crate::shell;

// ── Auto-layout ────────────────────────────────────────────────────────────

/// Assign initial canvas positions based on topological depth × column layout.
pub fn auto_layout(graph: &Graph) -> HashMap<NodeId, Pos2> {
    let order = stax_graph::topo_sort(graph);
    let mut depth: HashMap<NodeId, usize> = HashMap::new();

    for &nid in &order {
        let d = graph
            .predecessors(nid)
            .iter()
            .map(|p| depth.get(p).copied().unwrap_or(0) + 1)
            .max()
            .unwrap_or(0);
        depth.insert(nid, d);
    }

    let max_d = depth.values().copied().max().unwrap_or(0);
    let mut cols: Vec<Vec<NodeId>> = vec![Vec::new(); max_d + 1];
    for &nid in &order {
        cols[depth[&nid]].push(nid);
    }

    let col_w   = 180.0_f32;
    let row_h   = 90.0_f32;
    let margin  = 40.0_f32;
    let mut positions = HashMap::new();

    for (col, nids) in cols.iter().enumerate() {
        let total = nids.len() as f32;
        for (row, &nid) in nids.iter().enumerate() {
            // Center the column vertically if it has fewer rows
            let y_offset = (total - 1.0) * row_h * 0.5;
            positions.insert(
                nid,
                pos2(
                    margin + col as f32 * col_w,
                    margin + row as f32 * row_h - y_offset + 150.0,
                ),
            );
        }
    }
    positions
}

// ── Node geometry ──────────────────────────────────────────────────────────

pub fn node_label(node: &Node) -> String {
    node.label()
}

fn node_sublabel(node: &Node) -> Option<String> {
    if node.inputs.is_empty() { return None; }
    let labels: Vec<&str> = node.inputs.iter().map(|p| p.label.as_ref()).collect();
    Some(labels.join("  "))
}

/// Visual size of a node at zoom=1.  Ports protrude PORT_HALF above/below.
pub fn node_size(node: &Node) -> Vec2 {
    let label = node_label(node);
    let char_w = 7.2_f32;
    let min_w = shell::NODE_MIN_W.max(label.len() as f32 * char_w + 20.0);
    let has_sub = node_sublabel(node).is_some() || node.adverb.is_some();
    let h = shell::NODE_HDR_H + if has_sub { shell::NODE_SUB_H } else { 0.0 };
    vec2(min_w, h)
}

// ── Port position helpers ──────────────────────────────────────────────────

/// Screen position of a port center, given the node's top-left screen pos.
/// `is_output`: true → bottom row, false → top row.
pub fn port_screen_pos(node_screen: Pos2, node: &Node, port_idx: u8, is_output: bool) -> Pos2 {
    let sz  = node_size(node);
    let n   = if is_output { node.outputs.len() } else { node.inputs.len() } as f32;
    let idx = port_idx as f32;
    let x   = node_screen.x + sz.x * (idx + 1.0) / (n + 1.0);
    let y   = if is_output {
        node_screen.y + sz.y
    } else {
        node_screen.y
    };
    pos2(x, y)
}

// ── Painting primitives ────────────────────────────────────────────────────

/// Dot-grid background matching the canvas-wrap CSS pattern (24px × 24px, 0.6px dots).
pub fn draw_dot_grid(painter: &Painter, rect: Rect, pan: Vec2, zoom: f32) {
    let spacing = 24.0 * zoom;
    if spacing < 4.0 { return; } // too dense to draw

    let offset_x = (pan.x * zoom).rem_euclid(spacing);
    let offset_y = (pan.y * zoom).rem_euclid(spacing);

    let mut x = rect.min.x + offset_x;
    while x <= rect.max.x {
        let mut y = rect.min.y + offset_y;
        while y <= rect.max.y {
            painter.circle_filled(pos2(x, y), 0.7, shell::RULE_2);
            y += spacing;
        }
        x += spacing;
    }
}

/// Cubic-bezier wire from `from` (output port) to `to` (input port).
pub fn draw_wire(painter: &Painter, from: Pos2, to: Pos2, kind: &PortKind, zoom: f32) {
    let (color, dashed, width) = shell::port_style(kind);
    let w = width * zoom.sqrt();

    let dy = ((to.y - from.y).abs() * 0.5).max(40.0 * zoom);
    let c1 = pos2(from.x, from.y + dy);
    let c2 = pos2(to.x, to.y - dy);

    let pts: Vec<Pos2> = (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            pos2(
                u*u*u*from.x + 3.0*u*u*t*c1.x + 3.0*u*t*t*c2.x + t*t*t*to.x,
                u*u*u*from.y + 3.0*u*u*t*c1.y + 3.0*u*t*t*c2.y + t*t*t*to.y,
            )
        })
        .collect();

    let stroke = Stroke::new(w, color);
    if dashed {
        for chunk in pts.windows(2).enumerate().filter_map(|(i, s)| {
            if i % 2 == 0 { Some(s) } else { None }
        }) {
            painter.line_segment([chunk[0], chunk[1]], stroke);
        }
    } else {
        painter.add(egui::Shape::line(pts, stroke));
    }
}

/// Draw a port square (10×10 px in canvas space, scaled by zoom).
pub fn draw_port(painter: &Painter, center: Pos2, kind: &PortKind, zoom: f32) {
    let half = shell::PORT_HALF * zoom;
    let rect = Rect::from_center_size(center, vec2(half * 2.0, half * 2.0));

    let (color, dashed, _) = shell::port_style(kind);

    match kind {
        PortKind::Stream => {
            painter.rect_filled(rect, 0.0, shell::PAPER);
            if dashed {
                // Dotted border approximation: draw 8 short segments around the rect
                let segs = 8;
                let pts = [
                    rect.left_top(), rect.right_top(),
                    rect.right_top(), rect.right_bottom(),
                    rect.right_bottom(), rect.left_bottom(),
                    rect.left_bottom(), rect.left_top(),
                ];
                for pair in pts.chunks(2).take(segs / 2) {
                    // Draw only odd segments (skip the alternate ones for dashed effect)
                    painter.line_segment([pair[0], pair[1]], Stroke::new(1.0 * zoom, color));
                }
            } else {
                painter.rect_stroke(rect, 0.0, Stroke::new(1.0 * zoom, color), StrokeKind::Outside);
            }
        }
        PortKind::Fun => {
            painter.rect_filled(rect, 0.0, shell::PAPER);
            painter.rect_stroke(rect, 0.0, Stroke::new(1.5 * zoom, color), StrokeKind::Outside);
        }
        _ => {
            painter.rect_filled(rect, 0.0, color);
        }
    }
}

/// Draw a complete node.  Returns the node's screen rect (including port protrusion).
pub fn draw_node(
    painter: &Painter,
    screen_pos: Pos2,  // top-left of the node body
    node: &Node,
    selected: bool,
    hovered: bool,
    zoom: f32,
) -> Rect {
    let sz      = node_size(node) * zoom;
    let body    = Rect::from_min_size(screen_pos, sz);
    let label   = node_label(node);
    let is_sink = node.is_sink();

    // Background fill
    let fill = if is_sink { shell::SURFACE } else { shell::PAPER };
    painter.rect_filled(body, 0.0, fill);

    // Border
    let border_color = if selected { shell::WARM } else if hovered { shell::INK_2 } else { shell::INK };
    let border_w = if selected { 1.5 * zoom } else { 1.0 * zoom };
    painter.rect_stroke(body, 0.0, Stroke::new(border_w, border_color), StrokeKind::Outside);
    if selected {
        // Extra warm glow: a slightly expanded rect
        let glow = body.expand(1.0 * zoom);
        painter.rect_stroke(glow, 0.0, Stroke::new(0.5 * zoom, shell::WARM), StrokeKind::Outside);
    }

    // Header text (label + optional adverb badge)
    let font_id = egui::FontId::new(12.0 * zoom, egui::FontFamily::Monospace);
    let hdr_center = pos2(
        body.center().x,
        body.min.y + shell::NODE_HDR_H * zoom * 0.5,
    );

    if let Some(adv) = &node.adverb {
        // Adverb badge on the right
        let adv_str = match adv {
            stax_core::Adverb::Reduce    => "/",
            stax_core::Adverb::Scan      => "\\",
            stax_core::Adverb::Pairwise  => "^",
        };
        let adv_label = format!(" {adv_str}");
        let label_x = body.min.x + 10.0 * zoom;
        painter.text(
            pos2(label_x, hdr_center.y),
            egui::Align2::LEFT_CENTER,
            &label,
            font_id.clone(),
            shell::INK,
        );
        // Small bordered adverb badge
        let badge_text_pos = pos2(body.max.x - 24.0 * zoom, hdr_center.y);
        painter.text(
            badge_text_pos,
            egui::Align2::LEFT_CENTER,
            adv_label,
            egui::FontId::new(10.0 * zoom, egui::FontFamily::Monospace),
            shell::INK_2,
        );
    } else {
        painter.text(
            hdr_center,
            egui::Align2::CENTER_CENTER,
            &label,
            font_id.clone(),
            shell::INK,
        );
    }

    // Sub-label row (port name hints)
    if let Some(sub) = node_sublabel(node) {
        let sub_y = body.min.y + shell::NODE_HDR_H * zoom;
        let sub_rect = Rect::from_min_size(
            pos2(body.min.x, sub_y),
            vec2(sz.x, shell::NODE_SUB_H * zoom),
        );
        // Dotted top divider
        painter.line_segment(
            [sub_rect.left_top(), sub_rect.right_top()],
            Stroke::new(0.5 * zoom, shell::RULE_2),
        );
        painter.text(
            sub_rect.center(),
            egui::Align2::CENTER_CENTER,
            &sub,
            egui::FontId::new(10.0 * zoom, egui::FontFamily::Monospace),
            shell::INK_2,
        );
    }

    // Input ports (top)
    for (idx, port) in node.inputs.iter().enumerate() {
        let center = port_screen_pos(screen_pos, node, idx as u8, false);
        draw_port(painter, center, &port.kind, zoom);
    }

    // Output ports (bottom)
    for (idx, port) in node.outputs.iter().enumerate() {
        let center = port_screen_pos(screen_pos, node, idx as u8, true);
        draw_port(painter, center, &port.kind, zoom);
    }

    body
}

/// Draw the canvas-level view tab strip (floating in the top-left of the canvas).
pub fn draw_canvas_header(painter: &Painter, rect: Rect, view: crate::app::View) {
    let y = rect.min.y + 10.0;
    let mut x = rect.min.x + 12.0;
    let tabs = [
        (crate::app::View::Graph, "graph"),
        (crate::app::View::Text,  "text"),
    ];
    let font = egui::FontId::new(10.0, egui::FontFamily::Monospace);

    for (v, name) in tabs {
        let active = v == view;
        let tab_rect = Rect::from_min_size(pos2(x, y), vec2(name.len() as f32 * 6.5 + 16.0, 18.0));

        if active {
            painter.rect_filled(tab_rect, 0.0, shell::PAPER);
            painter.rect_stroke(tab_rect, 0.0, Stroke::new(1.0, shell::RULE), StrokeKind::Outside);
        } else {
            painter.rect_stroke(tab_rect, 0.0, Stroke::new(0.5, shell::RULE_2), StrokeKind::Outside);
        }

        painter.text(
            tab_rect.center(),
            egui::Align2::CENTER_CENTER,
            name.to_uppercase(),
            font.clone(),
            if active { shell::INK } else { shell::INK_3 },
        );
        x += tab_rect.width() + 4.0;
    }
}

/// Port-type legend in the bottom-left of the canvas.
pub fn draw_legend(painter: &Painter, rect: Rect) {
    let entries: &[(&str, PortKind)] = &[
        ("real",   PortKind::Real),
        ("signal", PortKind::Signal),
        ("stream", PortKind::Stream),
        ("fun",    PortKind::Fun),
        ("form",   PortKind::Form),
    ];

    let pad    = 8.0;
    let item_w = 54.0;
    let h      = 22.0;
    let total_w = entries.len() as f32 * item_w + pad * 2.0;
    let legend_rect = Rect::from_min_size(
        pos2(rect.min.x + 12.0, rect.max.y - h - 10.0),
        vec2(total_w, h),
    );

    painter.rect_filled(legend_rect, 0.0, shell::PAPER);
    painter.rect_stroke(legend_rect, 0.0, Stroke::new(1.0, shell::RULE), StrokeKind::Outside);

    let font = egui::FontId::new(10.0, egui::FontFamily::Monospace);
    let mut x = legend_rect.min.x + pad;
    let cy = legend_rect.center().y;

    for (label, kind) in entries {
        let (color, _, _) = shell::port_style(kind);
        let sq = Rect::from_center_size(pos2(x + 5.0, cy), vec2(10.0, 10.0));
        match kind {
            PortKind::Stream => {
                painter.rect_filled(sq, 0.0, shell::PAPER);
                painter.rect_stroke(sq, 0.0, Stroke::new(1.0, color), StrokeKind::Outside);
            }
            PortKind::Fun => {
                painter.rect_filled(sq, 0.0, shell::PAPER);
                painter.rect_stroke(sq, 0.0, Stroke::new(1.5, color), StrokeKind::Outside);
            }
            _ => { painter.rect_filled(sq, 0.0, color); }
        }
        painter.text(
            pos2(x + 18.0, cy),
            egui::Align2::LEFT_CENTER,
            *label,
            font.clone(),
            shell::INK_2,
        );
        x += item_w;
    }
}
