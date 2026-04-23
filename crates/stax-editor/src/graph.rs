use crate::shell;
use egui::{pos2, vec2, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};
use stax_graph::{Graph, Node, NodeId, PortKind};
use std::collections::HashMap;

// ── Public interaction types ───────────────────────────────────────────────

/// Per-node action returned from draw_node when a clickable badge is in the hit zone.
#[derive(Clone, Debug)]
pub enum NodeAction {
    /// port index; caller cycles rank_overrides[(node_id, idx)]
    CyclePortRank(u8),
    /// caller cycles adverb_overrides[node_id]
    CycleAdverb,
}

/// A clickable zone returned from draw_node together with the action it triggers.
pub struct NodeInteract {
    pub zone: Rect,
    pub action: NodeAction,
}

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

    let col_w = 180.0_f32;
    let row_h = 90.0_f32;
    let margin = 40.0_f32;
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
    if node.inputs.is_empty() {
        return None;
    }
    let labels: Vec<&str> = node.inputs.iter().map(|p| p.label.as_ref()).collect();
    Some(labels.join("  "))
}

const SCOPE_H: f32 = 36.0;

/// Full visual size of a node at zoom=1, optionally including scope strip height.
pub fn node_size_full(node: &Node, has_scope: bool) -> Vec2 {
    let label = node_label(node);
    let char_w = 7.2_f32;
    let min_w = shell::NODE_MIN_W.max(label.len() as f32 * char_w + 20.0);
    let has_sub = node_sublabel(node).is_some() || node.adverb.is_some();
    let mut h = shell::NODE_HDR_H + if has_sub { shell::NODE_SUB_H } else { 0.0 };
    if has_scope && node.is_sink() {
        h += SCOPE_H;
    }
    vec2(min_w, h)
}

/// Visual size of a node at zoom=1. Ports protrude PORT_HALF above/below.
pub fn node_size(node: &Node) -> Vec2 {
    node_size_full(node, false)
}

// ── Port position helpers ──────────────────────────────────────────────────

/// Screen position of a port center, given the node's top-left screen pos.
/// `is_output`: true → bottom row, false → top row.
pub fn port_screen_pos(node_screen: Pos2, node: &Node, port_idx: u8, is_output: bool) -> Pos2 {
    let sz = node_size(node);
    let n = if is_output {
        node.outputs.len()
    } else {
        node.inputs.len()
    } as f32;
    let idx = port_idx as f32;
    let x = node_screen.x + sz.x * (idx + 1.0) / (n + 1.0);
    let y = if is_output {
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
    if spacing < 4.0 {
        return;
    } // too dense to draw

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
                u * u * u * from.x
                    + 3.0 * u * u * t * c1.x
                    + 3.0 * u * t * t * c2.x
                    + t * t * t * to.x,
                u * u * u * from.y
                    + 3.0 * u * u * t * c1.y
                    + 3.0 * u * t * t * c2.y
                    + t * t * t * to.y,
            )
        })
        .collect();

    let stroke = Stroke::new(w, color);
    if dashed {
        for chunk in pts
            .windows(2)
            .enumerate()
            .filter_map(|(i, s)| if i % 2 == 0 { Some(s) } else { None })
        {
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
                    rect.left_top(),
                    rect.right_top(),
                    rect.right_top(),
                    rect.right_bottom(),
                    rect.right_bottom(),
                    rect.left_bottom(),
                    rect.left_bottom(),
                    rect.left_top(),
                ];
                for pair in pts.chunks(2).take(segs / 2) {
                    // Draw only odd segments (skip the alternate ones for dashed effect)
                    painter.line_segment([pair[0], pair[1]], Stroke::new(1.0 * zoom, color));
                }
            } else {
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0 * zoom, color),
                    StrokeKind::Outside,
                );
            }
        }
        PortKind::Fun => {
            painter.rect_filled(rect, 0.0, shell::PAPER);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.5 * zoom, color),
                StrokeKind::Outside,
            );
        }
        _ => {
            painter.rect_filled(rect, 0.0, color);
        }
    }
}

/// Draw a complete node.
///
/// Returns `(body_rect, interacts)` where `body_rect` is the node header+sub area
/// (without scope extension) suitable for drag hit-testing, and `interacts` lists
/// all clickable badge zones the caller should hit-test each frame.
#[allow(clippy::too_many_arguments)]
pub fn draw_node(
    painter: &Painter,
    screen_pos: Pos2,
    node: &Node,
    selected: bool,
    hovered: bool,
    zoom: f32,
    port_ranks: &[u8],
    adverb_override: Option<stax_core::Adverb>,
    scope_samples: Option<&[f32]>,
) -> (Rect, Vec<NodeInteract>) {
    let has_scope = scope_samples.is_some() && node.is_sink();

    // base_sz is the node body without scope; used for the returned body rect and
    // for positioning the scope strip below.
    let base_sz = node_size(node) * zoom;
    let full_sz = node_size_full(node, has_scope) * zoom;

    let body = Rect::from_min_size(screen_pos, base_sz);
    let full = Rect::from_min_size(screen_pos, full_sz);
    let label = node_label(node);
    let is_sink = node.is_sink();

    let mut interacts: Vec<NodeInteract> = Vec::new();

    // Background fill — covers the full rect (body + scope)
    let fill = if is_sink {
        shell::SURFACE
    } else {
        shell::PAPER
    };
    painter.rect_filled(full, 0.0, fill);

    // Border drawn around the full node rect
    let border_color = if selected {
        shell::WARM
    } else if hovered {
        shell::INK_2
    } else {
        shell::INK
    };
    let border_w = if selected { 1.5 * zoom } else { 1.0 * zoom };
    painter.rect_stroke(
        full,
        0.0,
        Stroke::new(border_w, border_color),
        StrokeKind::Outside,
    );
    if selected {
        let glow = full.expand(1.0 * zoom);
        painter.rect_stroke(
            glow,
            0.0,
            Stroke::new(0.5 * zoom, shell::WARM),
            StrokeKind::Outside,
        );
    }

    // ── Adverb toggle badge ────────────────────────────────────────────────
    // Unified rendering: uses adverb_override first, falls back to node.adverb.
    let eff_adv = adverb_override.or(node.adverb);

    let hdr_center = pos2(body.center().x, body.min.y + shell::NODE_HDR_H * zoom * 0.5);

    let (adv_text, adv_color) = match eff_adv {
        None => ("·", shell::INK_3),
        Some(stax_core::Adverb::Reduce) => ("/", shell::WARM),
        Some(stax_core::Adverb::Scan) => ("\\", shell::WARM),
        Some(stax_core::Adverb::Pairwise) => ("^", shell::WARM),
    };

    // Badge rect: 18×14 at right side of header
    let badge_x = body.max.x - 22.0 * zoom;
    let badge_rect =
        Rect::from_center_size(pos2(badge_x, hdr_center.y), vec2(18.0 * zoom, 14.0 * zoom));
    painter.rect_stroke(
        badge_rect,
        0.0,
        Stroke::new(0.8 * zoom, adv_color),
        StrokeKind::Outside,
    );
    painter.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        adv_text,
        egui::FontId::new(9.0 * zoom, egui::FontFamily::Monospace),
        adv_color,
    );
    interacts.push(NodeInteract {
        zone: badge_rect,
        action: NodeAction::CycleAdverb,
    });

    // ── Header label ───────────────────────────────────────────────────────
    let font_id = egui::FontId::new(12.0 * zoom, egui::FontFamily::Monospace);
    // Label sits left of the adverb badge; nudge left to avoid overlap
    let label_x = body.min.x + 10.0 * zoom;
    painter.text(
        pos2(label_x, hdr_center.y),
        egui::Align2::LEFT_CENTER,
        &label,
        font_id.clone(),
        shell::INK,
    );

    // ── Sub-label row (port name hints) ───────────────────────────────────
    if let Some(sub) = node_sublabel(node) {
        let sub_y = body.min.y + shell::NODE_HDR_H * zoom;
        let sub_rect = Rect::from_min_size(
            pos2(body.min.x, sub_y),
            vec2(base_sz.x, shell::NODE_SUB_H * zoom),
        );
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

    // ── Scope strip (sink nodes only) ──────────────────────────────────────
    if has_scope {
        let scope_rect = Rect::from_min_size(
            pos2(body.min.x, body.min.y + base_sz.y),
            vec2(full_sz.x, SCOPE_H * zoom),
        );
        // Thin divider above the scope
        painter.line_segment(
            [scope_rect.left_top(), scope_rect.right_top()],
            Stroke::new(0.5 * zoom, shell::RULE_2),
        );
        crate::scope::draw_scope(painter, scope_rect, scope_samples.unwrap_or(&[]));
    }

    // ── Input ports (top) + rank badges ───────────────────────────────────
    let badge_font = egui::FontId::new(8.0 * zoom.sqrt().max(0.4), egui::FontFamily::Monospace);
    for (idx, port) in node.inputs.iter().enumerate() {
        let center = port_screen_pos(screen_pos, node, idx as u8, false);
        draw_port(painter, center, &port.kind, zoom);

        // Rank badge just above-right of the port square
        let rank = port_ranks.get(idx).copied().unwrap_or(0);
        let badge_texts = ["·", "@", "@1", "@2", "@@"];
        let rank_text = badge_texts.get(rank as usize).copied().unwrap_or("·");
        let rank_color = if rank > 0 { shell::WARM } else { shell::INK_3 };
        let badge_center = pos2(center.x + 5.0 * zoom, center.y - 8.0 * zoom);
        painter.text(
            badge_center,
            egui::Align2::LEFT_CENTER,
            rank_text,
            badge_font.clone(),
            rank_color,
        );
        // 16×12 hit rect centered on the badge
        let rank_zone = Rect::from_center_size(badge_center, vec2(16.0 * zoom, 12.0 * zoom));
        interacts.push(NodeInteract {
            zone: rank_zone,
            action: NodeAction::CyclePortRank(idx as u8),
        });
    }

    // ── Output ports (bottom) ──────────────────────────────────────────────
    for (idx, port) in node.outputs.iter().enumerate() {
        let center = port_screen_pos(screen_pos, node, idx as u8, true);
        draw_port(painter, center, &port.kind, zoom);
    }

    (body, interacts)
}

/// Draw the canvas-level view tab strip (floating in the top-left of the canvas).
pub fn draw_canvas_header(painter: &Painter, rect: Rect, view: crate::app::View) {
    let y = rect.min.y + 10.0;
    let mut x = rect.min.x + 12.0;
    let tabs = [
        (crate::app::View::Graph, "graph"),
        (crate::app::View::Text, "text"),
    ];
    let font = egui::FontId::new(10.0, egui::FontFamily::Monospace);

    for (v, name) in tabs {
        let active = v == view;
        let tab_rect = Rect::from_min_size(pos2(x, y), vec2(name.len() as f32 * 6.5 + 16.0, 18.0));

        if active {
            painter.rect_filled(tab_rect, 0.0, shell::PAPER);
            painter.rect_stroke(
                tab_rect,
                0.0,
                Stroke::new(1.0, shell::RULE),
                StrokeKind::Outside,
            );
        } else {
            painter.rect_stroke(
                tab_rect,
                0.0,
                Stroke::new(0.5, shell::RULE_2),
                StrokeKind::Outside,
            );
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

// ── Wire hit-testing and ghost drawing ────────────────────────────────────────

/// Convert a screen position to canvas (world) coordinates.
pub fn screen_to_canvas(
    screen: egui::Pos2,
    pan: Vec2,
    zoom: f32,
    origin: egui::Pos2,
) -> egui::Pos2 {
    let rel = screen - origin;
    pos2(rel.x / zoom - pan.x, rel.y / zoom - pan.y)
}

/// Sample 24 points along the cubic bezier; return true if any point is within
/// `8.0 / zoom` pixels of `target`.
pub fn bezier_hit_test(from: Pos2, to: Pos2, target: Pos2, zoom: f32) -> bool {
    let dy = ((to.y - from.y).abs() * 0.5).max(40.0 * zoom);
    let c1 = pos2(from.x, from.y + dy);
    let c2 = pos2(to.x, to.y - dy);
    let threshold = (8.0 / zoom).max(4.0);
    for i in 0..=24 {
        let t = i as f32 / 24.0;
        let u = 1.0 - t;
        let pt = pos2(
            u * u * u * from.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * to.x,
            u * u * u * from.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * to.y,
        );
        if (pt - target).length() < threshold {
            return true;
        }
    }
    false
}

/// Return the port index on a node that is closest to `screen_pos` and within
/// twice PORT_HALF * zoom, for either inputs (`is_output=false`) or outputs.
pub fn port_at_screen(
    screen_pos: Pos2,
    node: &stax_graph::Node,
    node_screen: Pos2,
    zoom: f32,
    is_output: bool,
) -> Option<u8> {
    let port_count = if is_output {
        node.outputs.len()
    } else {
        node.inputs.len()
    };
    let threshold = shell::PORT_HALF * zoom * 4.0;
    for i in 0..port_count {
        let center = port_screen_pos(node_screen, node, i as u8, is_output);
        if (center - screen_pos).length() < threshold {
            return Some(i as u8);
        }
    }
    None
}

/// Draw a dashed ghost wire (in-progress connection preview).
pub fn draw_wire_ghost(painter: &Painter, from: Pos2, to: Pos2, zoom: f32) {
    let dy = ((to.y - from.y).abs() * 0.5).max(40.0 * zoom);
    let c1 = pos2(from.x, from.y + dy);
    let c2 = pos2(to.x, to.y - dy);
    let pts: Vec<Pos2> = (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            pos2(
                u * u * u * from.x
                    + 3.0 * u * u * t * c1.x
                    + 3.0 * u * t * t * c2.x
                    + t * t * t * to.x,
                u * u * u * from.y
                    + 3.0 * u * u * t * c1.y
                    + 3.0 * u * t * t * c2.y
                    + t * t * t * to.y,
            )
        })
        .collect();
    let stroke = egui::Stroke::new(1.5, shell::INK_2);
    for chunk in pts
        .windows(2)
        .enumerate()
        .filter_map(|(i, s)| if i % 2 == 0 { Some(s) } else { None })
    {
        painter.line_segment([chunk[0], chunk[1]], stroke);
    }
}

// ── D5: Word info helpers ──────────────────────────────────────────────────

/// Short description of a built-in word for the right-click context menu.
pub fn word_description(word: &str) -> Option<&'static str> {
    match word {
        "+" | "-" | "*" | "/" => Some("arithmetic binary op"),
        "pow" => Some("raise to power"),
        "sqrt" => Some("square root"),
        "abs" => Some("absolute value"),
        "neg" => Some("negate"),
        "%" | "mod" => Some("modulo"),
        "min" => Some("minimum of two values"),
        "max" => Some("maximum of two values"),
        "clip" => Some("clip to range [lo, hi]"),
        "floor" | "ceil" | "round" | "trunc" => Some("rounding function"),
        "sinosc" => Some("band-limited sine oscillator"),
        "saw" | "lfsaw" => Some("sawtooth oscillator"),
        "pulse" => Some("pulse / square oscillator"),
        "tri" => Some("triangle oscillator"),
        "wnoise" | "white" => Some("white noise generator"),
        "pink" | "pnoise" => Some("pink (1/f) noise"),
        "brown" => Some("brownian noise"),
        "lpf" | "lpf2" => Some("2-pole lowpass filter"),
        "hpf" | "hpf2" => Some("2-pole highpass filter"),
        "svflp" => Some("SVF lowpass (Chamberlin)"),
        "svfhp" => Some("SVF highpass (Chamberlin)"),
        "svfbp" => Some("SVF bandpass (Chamberlin)"),
        "rlpf" => Some("resonant lowpass filter"),
        "rhpf" => Some("resonant highpass filter"),
        "verb" => Some("FDN reverb (Jot/Hadamard)"),
        "pan2" => Some("stereo panning"),
        "play" => Some("send to audio output (sink)"),
        "stop" => Some("stop audio playback"),
        "p" => Some("print top of stack"),
        "trace" => Some("print with label"),
        "ar" => Some("attack-release envelope"),
        "adsr" => Some("4-stage ADSR envelope"),
        "ord" => Some("finite integer sequence 0..N"),
        "nat" => Some("infinite natural numbers 0,1,2,…"),
        "ordz" => Some("infinite 1,2,3,… sequence"),
        "by" => Some("arithmetic sequence (start step)"),
        "cyc" => Some("cycle a list infinitely"),
        "N" => Some("take N elements from stream"),
        "take" => Some("take first N elements"),
        "drop" => Some("drop first N elements"),
        "zip" => Some("interleave two streams"),
        "dup" => Some("duplicate top of stack"),
        "swap" => Some("swap top two stack items"),
        "over" => Some("copy second item to top"),
        "drop2" => Some("drop top stack item"),
        _ => None,
    }
}

/// Short arity string for a node, e.g. "2 in → 1 out".
pub fn node_arity_string(node: &Node) -> String {
    format!("{} in → {} out", node.inputs.len(), node.outputs.len())
}

/// Port type summary string, e.g. "in: real signal  out: signal".
pub fn node_port_type_string(node: &Node) -> String {
    let ins: Vec<&str> = node
        .inputs
        .iter()
        .map(|p| port_kind_name(&p.kind))
        .collect();
    let outs: Vec<&str> = node
        .outputs
        .iter()
        .map(|p| port_kind_name(&p.kind))
        .collect();
    if ins.is_empty() && outs.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    if !ins.is_empty() {
        s.push_str(&format!("in: {}  ", ins.join(" ")));
    }
    if !outs.is_empty() {
        s.push_str(&format!("out: {}", outs.join(" ")));
    }
    s
}

fn port_kind_name(kind: &PortKind) -> &'static str {
    match kind {
        PortKind::Real => "real",
        PortKind::Signal => "signal",
        PortKind::Stream => "stream",
        PortKind::Fun => "fun",
        PortKind::Form => "form",
        PortKind::Any => "any",
        PortKind::Str => "str",
        PortKind::Sym => "sym",
    }
}

/// Port-type legend in the bottom-left of the canvas.
pub fn draw_legend(painter: &Painter, rect: Rect) {
    let entries: &[(&str, PortKind)] = &[
        ("real", PortKind::Real),
        ("signal", PortKind::Signal),
        ("stream", PortKind::Stream),
        ("fun", PortKind::Fun),
        ("form", PortKind::Form),
    ];

    let pad = 8.0;
    let item_w = 54.0;
    let h = 22.0;
    let total_w = entries.len() as f32 * item_w + pad * 2.0;
    let legend_rect = Rect::from_min_size(
        pos2(rect.min.x + 12.0, rect.max.y - h - 10.0),
        vec2(total_w, h),
    );

    painter.rect_filled(legend_rect, 0.0, shell::PAPER);
    painter.rect_stroke(
        legend_rect,
        0.0,
        Stroke::new(1.0, shell::RULE),
        StrokeKind::Outside,
    );

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
            _ => {
                painter.rect_filled(sq, 0.0, color);
            }
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
