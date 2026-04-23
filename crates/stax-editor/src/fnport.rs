//! fn-port detail view — drill-in panel for a function-valued (violet) port
//! on a selected node.
//!
//! The view shows:
//!   - Header strip:  node label · " · fn" · port index badge
//!   - Signature row: "fn arity N · in:type → out:type"
//!   - Subgraph zone: dot-grid canvas with a placeholder λ body rectangle,
//!     input/output stub markers, pan/zoom (Alt+drag / scroll)
//!   - Toolbar strip: promote · unbind · open in tab · text

use std::collections::HashMap;
use egui::{Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};
use stax_graph::{Graph, NodeId, PortKind};
use crate::{app::View, app::StaxApp, shell};

// ── FnPortState ────────────────────────────────────────────────────────────

/// Persistent state for the fn-port detail view. Stored in `StaxApp`.
#[derive(Debug, Clone)]
pub struct FnPortState {
    /// Node whose function-valued output port is being inspected.
    pub selected_node: Option<NodeId>,
    /// Which output port index on that node is being inspected (0-based).
    pub selected_port: Option<u8>,
    /// Sub-graph canvas pan (in canvas units, same coordinate system as
    /// the main graph view).
    pub subgraph_pan: Vec2,
    /// Sub-graph canvas zoom level (1.0 = 100 %).
    pub subgraph_zoom: f32,
    /// Lazily-built subgraph from the MakeFun node's body ops.
    pub subgraph: Option<Graph>,
    /// Auto-layout positions for subgraph nodes.
    pub subgraph_positions: HashMap<NodeId, Pos2>,
    /// The NodeId for which the subgraph was last built (cache key).
    pub subgraph_for: Option<NodeId>,
}

impl Default for FnPortState {
    fn default() -> Self {
        Self {
            selected_node: None,
            selected_port: None,
            subgraph_pan: Vec2::ZERO,
            subgraph_zoom: 1.0,
            subgraph: None,
            subgraph_positions: HashMap::new(),
            subgraph_for: None,
        }
    }
}

// ── Public helper ──────────────────────────────────────────────────────────

/// Return the first `Fun`-typed output port of the selected node, or `None`.
///
/// Used by the main graph view to decide whether to offer a fn-port drill-in,
/// and by the fn-port view itself to locate what it should display.
pub fn fnport_node_for_view(graph: &Graph, selected: Option<NodeId>) -> Option<(NodeId, u8)> {
    let nid = selected?;
    let node = graph.node(nid)?;
    for (i, port) in node.outputs.iter().enumerate() {
        if port.kind == PortKind::Fun {
            return Some((nid, i as u8));
        }
    }
    None
}

// ── draw_fnport_view ───────────────────────────────────────────────────────

impl StaxApp {
    /// Render the fn-port detail view into the central panel `ui`.
    pub(crate) fn draw_fnport_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();

        // ── Resolve selected node / fun-port ──────────────────────────────

        let sel = self.fnport.selected_node
            // If fnport has no selection, fall back to the graph-view selection.
            .or(self.selected_node);

        let found = fnport_node_for_view(&self.graph, sel);

        // ── Placeholder when nothing is selected ──────────────────────────

        if found.is_none() {
            ui.painter().rect_filled(rect, 0.0, shell::PAPER);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "select a function-valued port in the graph view",
                egui::FontId::new(12.0, egui::FontFamily::Monospace),
                shell::INK_3,
            );
            return;
        }

        let (nid, port_idx) = found.unwrap();

        // Sync fnport state so it remembers this selection.
        self.fnport.selected_node = Some(nid);
        self.fnport.selected_port = Some(port_idx);

        // Grab node data we need (cloned to avoid borrow conflicts).
        let (node_label, n_inputs, n_outputs, fun_arity, adverb_str, body_ops) = {
            if let Some(node) = self.graph.node(nid) {
                let label = node.label();
                let n_in  = node.inputs.len();
                let n_out = node.outputs.len();
                let (arity, ops) = match &node.kind {
                    stax_graph::NodeKind::MakeFun { params, body } => {
                        (format!("{}", params.len()), Some(body.to_vec()))
                    }
                    _ => ("?".to_owned(), None),
                };
                let adv = node.adverb.map(|a| match a {
                    stax_core::Adverb::Reduce   => "/",
                    stax_core::Adverb::Scan     => "\\",
                    stax_core::Adverb::Pairwise => "^",
                });
                (label, n_in, n_out, arity, adv, ops)
            } else {
                return;
            }
        };

        // Build or cache the subgraph from the MakeFun body.
        if self.fnport.subgraph_for != Some(nid) {
            if let Some(ref ops) = body_ops {
                let sub = stax_graph::lift(ops);
                self.fnport.subgraph_positions = crate::graph::auto_layout(&sub);
                self.fnport.subgraph = Some(sub);
            } else {
                self.fnport.subgraph = None;
                self.fnport.subgraph_positions.clear();
            }
            self.fnport.subgraph_for = Some(nid);
        }

        // ── Layout bands ──────────────────────────────────────────────────

        const HDR_H:     f32 = 32.0;
        const SIG_H:     f32 = 22.0;
        const TOOLBAR_H: f32 = 32.0;

        let hdr_rect  = Rect::from_min_size(rect.min, vec2(rect.width(), HDR_H));
        let sig_rect  = Rect::from_min_size(
            pos2(rect.min.x, rect.min.y + HDR_H),
            vec2(rect.width(), SIG_H),
        );
        let toolbar_rect = Rect::from_min_size(
            pos2(rect.min.x, rect.max.y - TOOLBAR_H),
            vec2(rect.width(), TOOLBAR_H),
        );
        let canvas_rect = Rect::from_min_max(
            pos2(rect.min.x, rect.min.y + HDR_H + SIG_H),
            pos2(rect.max.x, rect.max.y - TOOLBAR_H),
        );

        // Clone the painter so we don't hold a &ui borrow across allocate_rect calls.
        let painter = ui.painter().clone();

        // ── Header strip ──────────────────────────────────────────────────

        painter.rect_filled(hdr_rect, 0.0, shell::PAPER);
        // Bottom border of header
        painter.line_segment(
            [hdr_rect.left_bottom(), hdr_rect.right_bottom()],
            Stroke::new(1.0, shell::RULE),
        );

        // "NodeLabel" in WARM
        let mut x = hdr_rect.min.x + 14.0;
        let cy    = hdr_rect.center().y;

        let label_galley = painter.layout_no_wrap(
            node_label.clone(),
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
            shell::WARM,
        );
        painter.galley(pos2(x, cy - label_galley.size().y * 0.5), label_galley.clone(), shell::WARM);
        x += label_galley.size().x + 2.0;

        // " · fn" in INK_2
        let suffix_str = " · fn";
        let suffix_galley = painter.layout_no_wrap(
            suffix_str.to_owned(),
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
            shell::INK_2,
        );
        painter.galley(pos2(x, cy - suffix_galley.size().y * 0.5), suffix_galley.clone(), shell::INK_2);
        x += suffix_galley.size().x + 6.0;

        // Port index badge: small bordered rect  "out:N"
        let badge_str = format!("out:{port_idx}");
        let badge_galley = painter.layout_no_wrap(
            badge_str.clone(),
            egui::FontId::new(10.0, egui::FontFamily::Monospace),
            shell::PORT_FUN,
        );
        let badge_w = badge_galley.size().x + 8.0;
        let badge_h = 14.0;
        let badge_rect = Rect::from_min_size(
            pos2(x, cy - badge_h * 0.5),
            vec2(badge_w, badge_h),
        );
        painter.rect_filled(badge_rect, 0.0, shell::PAPER);
        painter.rect_stroke(badge_rect, 0.0, Stroke::new(1.0, shell::PORT_FUN), StrokeKind::Outside);
        painter.galley(
            pos2(badge_rect.min.x + 4.0, cy - badge_galley.size().y * 0.5),
            badge_galley,
            shell::PORT_FUN,
        );

        // Optional adverb indicator at right
        if let Some(adv) = adverb_str {
            painter.text(
                pos2(hdr_rect.max.x - 14.0, cy),
                egui::Align2::RIGHT_CENTER,
                adv,
                egui::FontId::new(12.0, egui::FontFamily::Monospace),
                shell::WARM,
            );
        }

        // ── Signature row ─────────────────────────────────────────────────

        painter.rect_filled(sig_rect, 0.0, shell::PAPER);
        // Dotted bottom border
        let seg_w = 4.0_f32;
        let mut dx = sig_rect.min.x;
        let dot_y  = sig_rect.max.y - 0.5;
        while dx < sig_rect.max.x {
            let end = (dx + seg_w).min(sig_rect.max.x);
            painter.line_segment(
                [pos2(dx, dot_y), pos2(end, dot_y)],
                Stroke::new(0.5, shell::RULE_2),
            );
            dx += seg_w * 2.0;
        }

        let sig_text = format!(
            "fn arity {fun_arity}  ·  in:{n_inputs}  →  out:{n_outputs}"
        );
        painter.text(
            pos2(sig_rect.min.x + 14.0, sig_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &sig_text,
            egui::FontId::new(11.0, egui::FontFamily::Monospace),
            shell::INK_2,
        );

        // ── Subgraph zone ─────────────────────────────────────────────────

        // Background
        painter.rect_filled(canvas_rect, 0.0, shell::SURFACE);

        // Dot-grid (clip to canvas rect)
        let clip_id = ui.id().with("fnport_clip");
        let clip_rect = canvas_rect;
        {
            let clip_painter = painter.with_clip_rect(clip_rect);
            crate::graph::draw_dot_grid(
                &clip_painter,
                canvas_rect,
                self.fnport.subgraph_pan,
                self.fnport.subgraph_zoom,
            );

            let pan  = self.fnport.subgraph_pan;
            let zoom = self.fnport.subgraph_zoom;
            let origin = canvas_rect.min;

            // Canvas → screen helper
            let to_screen = |p: Pos2| -> Pos2 {
                origin + (vec2(p.x, p.y) + pan) * zoom
            };

            if let Some(ref sub) = self.fnport.subgraph.clone() {
                // ── Real subgraph rendering ───────────────────────────────

                let subpos = &self.fnport.subgraph_positions;

                // Draw edges first (under nodes)
                for edge in sub.edges() {
                    if let (Some(src_node), Some(dst_node)) = (sub.node(edge.src.node), sub.node(edge.dst.node)) {
                        let src_canvas = subpos.get(&edge.src.node).copied().unwrap_or(pos2(0.0, 0.0));
                        let dst_canvas = subpos.get(&edge.dst.node).copied().unwrap_or(pos2(0.0, 0.0));
                        let src_sz = crate::graph::node_size(src_node);
                        let dst_sz = crate::graph::node_size(dst_node);
                        let from_s = to_screen(pos2(src_canvas.x + src_sz.x, src_canvas.y + src_sz.y * 0.5));
                        let to_s   = to_screen(pos2(dst_canvas.x, dst_canvas.y + dst_sz.y * 0.5));
                        let kind = src_node.outputs.get(edge.src.port as usize)
                            .map(|p| &p.kind).unwrap_or(&PortKind::Real);
                        crate::graph::draw_wire(&clip_painter, from_s, to_s, kind, zoom);
                    }
                }

                // Draw nodes
                for node in sub.nodes_in_order() {
                    let canvas_pos = subpos.get(&node.id).copied().unwrap_or(pos2(0.0, 0.0));
                    let screen_pos = to_screen(canvas_pos);
                    crate::graph::draw_node(
                        &clip_painter, screen_pos, node,
                        false, false, zoom, &[], None, None,
                    );
                }
            } else {
                // ── Fallback: λ body placeholder ──────────────────────────
                let body_canvas_w = 160.0_f32;
                let body_canvas_h = 80.0_f32;
                let body_tl = pos2(-body_canvas_w * 0.5, -body_canvas_h * 0.5);
                let body_br = pos2( body_canvas_w * 0.5,  body_canvas_h * 0.5);
                let body_screen = Rect::from_min_max(to_screen(body_tl), to_screen(body_br));
                draw_dashed_rect(&clip_painter, body_screen, shell::PORT_FUN, 1.5 * zoom.sqrt(), 6.0, 4.0);
                clip_painter.text(
                    body_screen.center(), egui::Align2::CENTER_CENTER, "λ body",
                    egui::FontId::new(12.0 * zoom.sqrt().max(0.6), egui::FontFamily::Monospace),
                    shell::INK_3,
                );
            }
        }

        // Canvas border
        painter.rect_stroke(canvas_rect, 0.0, Stroke::new(1.0, shell::RULE), StrokeKind::Outside);

        // ── Subgraph interaction (pan/zoom) ───────────────────────────────

        let canvas_resp = ui.allocate_rect(canvas_rect, egui::Sense::click_and_drag());
        let _ = clip_id; // suppress unused warning

        if canvas_resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let old_z = self.fnport.subgraph_zoom;
                let new_z = (old_z * (1.0 + scroll * 0.0015)).clamp(0.15, 5.0);
                if let Some(cursor) = ui.input(|i| i.pointer.hover_pos()) {
                    let origin = canvas_rect.min;
                    self.fnport.subgraph_pan +=
                        (cursor - origin) * (1.0 / new_z - 1.0 / old_z);
                }
                self.fnport.subgraph_zoom = new_z;
            }
        }

        let alt = ui.input(|i| i.modifiers.alt);
        if canvas_resp.dragged_by(egui::PointerButton::Middle)
            || (alt && canvas_resp.dragged_by(egui::PointerButton::Primary))
        {
            self.fnport.subgraph_pan +=
                canvas_resp.drag_delta() / self.fnport.subgraph_zoom;
        }

        // ── Toolbar strip ─────────────────────────────────────────────────

        painter.rect_filled(toolbar_rect, 0.0, shell::PAPER);
        painter.line_segment(
            [toolbar_rect.left_top(), toolbar_rect.right_top()],
            Stroke::new(1.0, shell::RULE),
        );

        // Toolbar buttons rendered as small bordered label rects.
        // We need mutable access for the "open in tab" click, so we track which
        // button was clicked by first doing a read pass, then acting.
        let buttons: &[&str] = &["promote", "unbind", "open in tab", "text"];
        let btn_h   = 18.0_f32;
        let btn_pad = 10.0_f32;
        let btn_gap = 6.0_f32;
        let mut bx  = toolbar_rect.min.x + 14.0;
        let btn_cy  = toolbar_rect.center().y;

        // Measure button widths and collect rects first (no allocation yet)
        let font = egui::FontId::new(11.0, egui::FontFamily::Monospace);
        let mut btn_rects: Vec<Rect> = Vec::with_capacity(buttons.len());
        for label in buttons.iter() {
            let w = label.len() as f32 * 6.5 + btn_pad * 2.0;
            btn_rects.push(Rect::from_min_size(
                pos2(bx, btn_cy - btn_h * 0.5),
                vec2(w, btn_h),
            ));
            bx += w + btn_gap;
        }

        // Determine hover / click state from stored pointer position.
        let ptr_pos = ui.input(|i| i.pointer.hover_pos());
        let clicked = ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary));

        let mut open_in_tab_clicked = false;
        for (i, (label, brect)) in buttons.iter().zip(btn_rects.iter()).enumerate() {
            let hovered = ptr_pos.is_some_and(|p| brect.contains(p));
            let bg   = if hovered { shell::PAPER_2 } else { shell::PAPER };
            let fg   = if hovered { shell::INK }     else { shell::INK_2 };
            let bord = if hovered { shell::INK_2 }   else { shell::RULE };
            painter.rect_filled(*brect, 0.0, bg);
            painter.rect_stroke(*brect, 0.0, Stroke::new(1.0, bord), StrokeKind::Outside);
            painter.text(
                brect.center(),
                egui::Align2::CENTER_CENTER,
                *label,
                font.clone(),
                fg,
            );
            if hovered && clicked && i == 2 {
                // "open in tab" — index 2
                open_in_tab_clicked = true;
            }
        }

        if open_in_tab_clicked {
            // Sync graph-view selection to the node we're inspecting, then switch.
            self.selected_node = Some(nid);
            self.view = View::Graph;
        }

        // "text" button (index 3) — jump to text view and try to show the MakeFun source line.
        let text_clicked = ptr_pos.is_some_and(|p| btn_rects.get(3).is_some_and(|r| r.contains(p)))
            && clicked;
        if text_clicked {
            self.view = View::Text;
        }
    }
}

// ── Private helpers ────────────────────────────────────────────────────────

/// Draw a dashed rectangle border.
///
/// `dash` — length of each on-segment in screen px.
/// `gap`  — length of each off-segment in screen px.
fn draw_dashed_rect(
    painter: &egui::Painter,
    rect: Rect,
    color: egui::Color32,
    width: f32,
    dash: f32,
    gap: f32,
) {
    // Walk all four edges and emit dash segments.
    let stroke = Stroke::new(width, color);
    let edges: [(Pos2, Pos2); 4] = [
        (rect.left_top(),    rect.right_top()),
        (rect.right_top(),   rect.right_bottom()),
        (rect.right_bottom(),rect.left_bottom()),
        (rect.left_bottom(), rect.left_top()),
    ];
    for (a, b) in edges {
        draw_dashed_line(painter, a, b, stroke, dash, gap);
    }
}

/// Draw a dashed line from `a` to `b`.
fn draw_dashed_line(
    painter: &egui::Painter,
    a: Pos2,
    b: Pos2,
    stroke: Stroke,
    dash: f32,
    gap: f32,
) {
    let total = (b - a).length();
    if total < 0.5 { return; }
    let dir = (b - a) / total;
    let period = dash + gap;
    let mut t = 0.0_f32;
    while t < total {
        let t_end = (t + dash).min(total);
        painter.line_segment([a + dir * t, a + dir * t_end], stroke);
        t += period;
    }
}
