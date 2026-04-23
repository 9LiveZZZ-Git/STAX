use std::collections::HashMap;
use egui::{pos2, vec2, Pos2, Rect, Stroke, Vec2};
use stax_core::Op;
use stax_eval::Interp;
use stax_graph::{Graph, NodeId};
use crate::shell;

// ── View enum ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View { Graph, Text, FnPort }

// ── REPL line ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplKind { Input, Output, Ok, Err }

#[derive(Debug, Clone)]
pub struct ReplLine {
    pub kind: ReplKind,
    pub text: String,
}

// ── App state ──────────────────────────────────────────────────────────────

pub struct StaxApp {
    // View
    pub view: View,

    // Source
    pub source: String,
    pub source_modified: bool,

    // Compiled IR (derived from source)
    pub ops: Vec<Op>,
    pub graph: Graph,
    pub parse_error: Option<String>,

    // Interpreter
    pub interp: Interp,
    pub repl_input: String,
    pub repl_history: Vec<ReplLine>,

    // Graph canvas state
    pub canvas_pan: Vec2,
    pub canvas_zoom: f32,
    pub node_positions: HashMap<NodeId, Pos2>,
    pub selected_node: Option<NodeId>,
    pub dragging: Option<NodeId>,

    // Text view state
    pub cursor_line: usize,

    // Animation
    pub anim_t: f32,
}

// ── Constructor ────────────────────────────────────────────────────────────

impl StaxApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Apply design-token visuals
        let mut vis = egui::Visuals::light();
        vis.panel_fill            = shell::PAPER;
        vis.window_fill           = shell::PAPER;
        vis.override_text_color   = Some(shell::INK);
        vis.extreme_bg_color      = shell::PAPER_2;
        vis.faint_bg_color        = shell::PAPER_2;
        vis.widgets.noninteractive.bg_fill     = shell::PAPER;
        vis.widgets.noninteractive.fg_stroke   = Stroke::new(1.0, shell::RULE);
        vis.widgets.inactive.bg_fill           = shell::PAPER;
        vis.widgets.hovered.bg_fill            = shell::PAPER_2;
        vis.widgets.active.bg_fill             = shell::SURFACE;
        vis.selection.bg_fill = egui::Color32::from_rgba_premultiplied(201, 72, 32, 40);
        vis.selection.stroke  = Stroke::new(1.0, shell::WARM);
        vis.window_shadow     = egui::epaint::Shadow::NONE;
        vis.popup_shadow      = egui::epaint::Shadow::NONE;
        cc.egui_ctx.set_visuals(vis);

        // Optional: load JetBrains Mono from assets dir if present
        if let Ok(bytes) = std::fs::read("assets/JetBrainsMono-Regular.ttf") {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "JetBrainsMono".into(),
                egui::FontData::from_owned(bytes).into(),
            );
            fonts.families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "JetBrainsMono".into());
            cc.egui_ctx.set_fonts(fonts);
        }

        let source = DEFAULT_SOURCE.to_owned();
        let mut app = Self {
            view: View::Graph,
            source_modified: false,
            ops: Vec::new(),
            graph: Graph::new(),
            parse_error: None,
            interp: Interp::new(),
            repl_input: String::new(),
            repl_history: Vec::new(),
            canvas_pan: Vec2::ZERO,
            canvas_zoom: 1.0,
            node_positions: HashMap::new(),
            selected_node: None,
            dragging: None,
            cursor_line: 1,
            anim_t: 0.0,
            source,
        };
        app.recompile();
        app
    }

    // ── Recompile source → ops → graph → layout ────────────────────────────

    pub fn recompile(&mut self) {
        match stax_parser::parse(&self.source) {
            Ok(ops) => {
                self.parse_error = None;
                self.graph = stax_graph::lift(&ops);
                // Preserve existing user positions; add new ones via auto-layout
                let new_positions = crate::graph::auto_layout(&self.graph);
                for (id, pos) in new_positions {
                    self.node_positions.entry(id).or_insert(pos);
                }
                self.ops = ops;
            }
            Err(e) => {
                self.parse_error = Some(e.to_string());
            }
        }
    }

    // ── REPL execution ─────────────────────────────────────────────────────

    pub fn exec_repl(&mut self, line: &str) {
        self.repl_history.push(ReplLine { kind: ReplKind::Input, text: line.to_owned() });

        // Special REPL commands
        match line.trim() {
            ".s" => {
                if self.interp.stack.is_empty() {
                    self.repl_push(ReplKind::Output, "stack: []");
                } else {
                    let lines: Vec<String> = self.interp.stack.iter().rev().enumerate()
                        .map(|(i, v)| format!("{i}: {}", crate::text::format_value_pub(v)))
                        .collect();
                    for s in lines {
                        self.repl_push(ReplKind::Output, &s);
                    }
                }
                return;
            }
            ".c" => {
                self.interp.stack.clear();
                self.repl_push(ReplKind::Ok, "stack cleared");
                return;
            }
            ".q" => {
                self.repl_push(ReplKind::Ok, "use window × to quit");
                return;
            }
            _ => {}
        }

        // Parse and execute
        match stax_parser::parse(line) {
            Err(e) => {
                self.repl_push(ReplKind::Err, &format!("parse error: {e}"));
            }
            Ok(ops) => {
                match self.interp.exec(&ops) {
                    Err(e) => {
                        self.repl_push(ReplKind::Err, &format!("{e}"));
                    }
                    Ok(()) => {
                        // Show top of stack if non-empty
                        if let Some(top) = self.interp.stack.last() {
                            let s = format!("⇒ {}", crate::text::format_value_pub(top));
                            self.repl_push(ReplKind::Ok, &s);
                        } else {
                            self.repl_push(ReplKind::Ok, "ok");
                        }
                    }
                }
            }
        }

        // If source changed via bind, update graph
        // (Not auto-triggered here; user can press ⌘R or type .compile)
    }

    fn repl_push(&mut self, kind: ReplKind, text: &str) {
        self.repl_history.push(ReplLine { kind, text: text.to_owned() });
        // Keep last 500 lines
        if self.repl_history.len() > 500 {
            self.repl_history.drain(0..100);
        }
    }
}

// ── eframe::App ────────────────────────────────────────────────────────────

impl eframe::App for StaxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.anim_t = ctx.input(|i| i.time as f32);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        let frame_none = egui::Frame::new()
            .fill(shell::PAPER)
            .inner_margin(egui::Margin::ZERO);

        // ── Header (40px, always) ──────────────────────────────────────────
        egui::TopBottomPanel::top("header")
            .exact_height(shell::HEADER_H)
            .frame(frame_none.fill(shell::PAPER))
            .show_separator_line(false)
            .show(ctx, |ui| self.draw_header(ui));

        // ── Tabs (28px, always) ───────────────────────────────────────────
        egui::TopBottomPanel::top("tabs")
            .exact_height(shell::TABS_H)
            .frame(frame_none.fill(shell::PAPER_2))
            .show_separator_line(false)
            .show(ctx, |ui| self.draw_tabs(ui));

        // ── Bottom bar ─────────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("botbar")
            .exact_height(shell::BOTBAR_H)
            .frame(frame_none.fill(shell::PAPER))
            .show_separator_line(false)
            .show(ctx, |ui| self.draw_botbar(ui));

        // ── Graph-view extras: REPL (120px) + time-travel (34px) ──────────
        if matches!(self.view, View::Graph | View::FnPort) {
            egui::TopBottomPanel::bottom("repl_panel")
                .exact_height(shell::REPL_H)
                .frame(frame_none)
                .show_separator_line(false)
                .show(ctx, |ui| self.draw_graph_repl(ui));

            egui::TopBottomPanel::bottom("timebar")
                .exact_height(shell::TIMEBAR_H)
                .frame(frame_none)
                .show_separator_line(false)
                .show(ctx, |ui| self.draw_timebar(ui));
        }

        // ── View-specific side panels & central area ───────────────────────
        match self.view {
            View::Graph | View::FnPort => {
                egui::SidePanel::left("library")
                    .exact_width(shell::LIB_W)
                    .frame(frame_none)
                    .show_separator_line(false)
                    .show(ctx, |ui| self.draw_library(ui));

                egui::SidePanel::right("inspector")
                    .exact_width(shell::INSP_W)
                    .frame(frame_none)
                    .show_separator_line(false)
                    .show(ctx, |ui| self.draw_inspector(ui));

                egui::CentralPanel::default()
                    .frame(frame_none)
                    .show(ctx, |ui| self.draw_graph_canvas(ui));
            }
            View::Text => {
                egui::SidePanel::left("files_panel")
                    .exact_width(shell::LIB_W)
                    .frame(frame_none)
                    .show_separator_line(false)
                    .show(ctx, |ui| self.draw_files_panel(ui));

                egui::SidePanel::right("side_panel")
                    .exact_width(shell::SIDE_W)
                    .frame(frame_none)
                    .show_separator_line(false)
                    .show(ctx, |ui| self.draw_text_side(ui));

                egui::CentralPanel::default()
                    .frame(frame_none)
                    .show(ctx, |ui| self.draw_text_editor(ui));
            }
        }
    }
}

// ── Shell chrome ───────────────────────────────────────────────────────────

impl StaxApp {
    fn draw_header(&self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        // Bottom border
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, shell::RULE),
        );

        ui.add_space(0.0);
        ui.horizontal(|ui| {
            ui.set_min_height(shell::HEADER_H);
            ui.add_space(14.0);

            // Brand
            ui.label(
                egui::RichText::new("STAX")
                    .color(shell::INK)
                    .size(12.0)
                    .monospace()
                    .strong(),
            );

            // Vertical divider
            let sep_rect = ui.allocate_space(vec2(1.0, 20.0)).1;
            ui.painter().line_segment(
                [sep_rect.center_top(), sep_rect.center_bottom()],
                Stroke::new(1.0, shell::RULE),
            );
            ui.add_space(14.0);

            // Menu
            for name in ["file", "edit", "view", "run", "help"] {
                ui.label(
                    egui::RichText::new(name).color(shell::INK_2).size(12.0).monospace(),
                );
                ui.add_space(6.0);
            }

            // Right-aligned: audio status + transport
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);

                // Transport
                let sep = ui.allocate_space(vec2(1.0, 20.0)).1;
                ui.painter().line_segment(
                    [sep.center_top(), sep.center_bottom()],
                    Stroke::new(1.0, shell::RULE),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("■ stop").color(shell::WARM).size(12.0).monospace(),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("▶ play").color(shell::INK).size(12.0).monospace(),
                );
                ui.add_space(14.0);

                // Audio stat
                let blink = (self.anim_t * 0.667).fract() < 0.5;
                let dot = if blink { "●" } else { "○" };
                ui.label(
                    egui::RichText::new(format!("{dot}  audio · 48 kHz · 128"))
                        .color(shell::INK_2)
                        .size(11.0)
                        .monospace(),
                );
            });
        });
    }

    fn draw_tabs(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER_2);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, shell::RULE),
        );

        ui.horizontal(|ui| {
            ui.set_min_height(shell::TABS_H);
            ui.add_space(14.0);

            for (view, label) in [
                (View::Graph,  "graph"),
                (View::Text,   "text"),
                (View::FnPort, "fn-port"),
            ] {
                let active = self.view == view;
                let resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(label.to_uppercase())
                            .color(if active { shell::INK } else { shell::INK_3 })
                            .size(11.0)
                            .monospace(),
                    )
                    .sense(egui::Sense::click()),
                );

                if active {
                    // Warm underline flush to bottom
                    let r = resp.rect;
                    ui.painter().line_segment(
                        [
                            pos2(r.min.x, rect.max.y - 1.0),
                            pos2(r.max.x, rect.max.y - 1.0),
                        ],
                        Stroke::new(1.5, shell::WARM),
                    );
                }

                if resp.clicked() {
                    self.view = view;
                }

                ui.add_space(8.0);

                // Dotted right divider between tabs
                let sep = ui.allocate_space(vec2(1.0, 14.0)).1;
                ui.painter().line_segment(
                    [sep.center_top(), sep.center_bottom()],
                    Stroke::new(0.5, shell::RULE_2),
                );
                ui.add_space(8.0);
            }

            // Right-aligned meta
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                let node_count = self.graph.node_count();
                let edge_count = self.graph.edge_count();
                ui.label(
                    egui::RichText::new(format!(
                        "{node_count} nodes · {edge_count} edges · patch.stax"
                    ))
                    .color(shell::INK_3)
                    .size(10.0)
                    .monospace(),
                );
            });
        });
    }

    fn draw_botbar(&self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        ui.painter().line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, shell::RULE),
        );

        ui.horizontal(|ui| {
            ui.set_min_height(shell::BOTBAR_H);
            ui.add_space(14.0);

            let status = if self.parse_error.is_some() {
                ("✕  parse error", shell::WARM)
            } else {
                ("✓  ready", shell::COOL)
            };
            ui.label(
                egui::RichText::new(status.0)
                    .color(status.1)
                    .size(11.0)
                    .monospace(),
            );
        });
    }

    // ── Graph view: bottom REPL section ────────────────────────────────────

    fn draw_graph_repl(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        ui.painter().line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, shell::RULE),
        );

        // "REPL" label in top-right corner
        ui.painter().text(
            pos2(rect.max.x - 14.0, rect.min.y + 6.0),
            egui::Align2::RIGHT_TOP,
            "REPL",
            egui::FontId::new(10.0, egui::FontFamily::Monospace),
            shell::INK_3,
        );

        let history_h = shell::REPL_H - 28.0;
        let history_rect = Rect::from_min_size(rect.min, vec2(rect.width(), history_h));
        let input_rect  = Rect::from_min_size(
            pos2(rect.min.x, rect.min.y + history_h),
            vec2(rect.width(), 28.0),
        );

        // History
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(history_rect).layout(egui::Layout::top_down(egui::Align::LEFT)));
        egui::ScrollArea::vertical()
            .id_salt("graph_repl")
            .max_height(history_h)
            .stick_to_bottom(true)
            .show(&mut child, |ui| {
                let history = self.repl_history.clone();
                for entry in &history {
                    let (prefix, color) = match entry.kind {
                        ReplKind::Input  => ("›  ", shell::INK),
                        ReplKind::Output => ("   ", shell::INK_2),
                        ReplKind::Ok     => ("   ", shell::COOL),
                        ReplKind::Err    => ("   ", shell::WARM),
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(
                            egui::RichText::new(format!("{}{}", prefix, entry.text))
                                .color(color)
                                .size(12.0)
                                .monospace(),
                        );
                    });
                }
            });

        // Input line
        let mut input_ui = ui.new_child(egui::UiBuilder::new().max_rect(input_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        input_ui.add_space(14.0);
        input_ui.label(egui::RichText::new("›  ").color(shell::INK_3).size(12.0).monospace());
        let resp = input_ui.add(
            egui::TextEdit::singleline(&mut self.repl_input)
                .font(egui::FontId::new(12.0, egui::FontFamily::Monospace))
                .text_color(shell::INK)
                .frame(false)
                .desired_width(f32::INFINITY),
        );
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let line = std::mem::take(&mut self.repl_input);
            if !line.trim().is_empty() {
                self.exec_repl(&line);
            }
            resp.request_focus();
        }
    }

    fn draw_timebar(&self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        ui.painter().line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, shell::RULE),
        );

        ui.horizontal(|ui| {
            ui.set_min_height(shell::TIMEBAR_H);
            ui.add_space(14.0);

            // Playback controls (visual only for M5)
            for ctrl in ["◀◀", "◀", "❚❚", "▶", "▶▶"] {
                ui.label(
                    egui::RichText::new(ctrl).color(shell::INK).size(13.0).monospace(),
                );
                ui.add_space(4.0);
            }

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("step — / —")
                    .color(shell::INK_2)
                    .size(11.0)
                    .monospace(),
            );

            // Scrub bar
            ui.add_space(8.0);
            let bar_w = ui.available_width() - 120.0;
            let bar_rect = ui.allocate_space(vec2(bar_w, 6.0)).1;
            ui.painter().rect_filled(bar_rect, 0.0, shell::SURFACE);
            ui.painter().rect_stroke(bar_rect, 0.0, Stroke::new(1.0, shell::RULE_2), egui::StrokeKind::Outside);
        });
    }

    // ── Library panel ───────────────────────────────────────────────────────

    pub(crate) fn draw_library(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        ui.painter().line_segment(
            [rect.right_top(), rect.right_bottom()],
            Stroke::new(1.0, shell::RULE),
        );

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_width(shell::LIB_W);
            ui.add_space(14.0);

            lib_header(ui, "library");

            lib_group(ui, "math", &["+","-","×","÷","%","pow","sqrt","abs","neg","to","ord","nat"]);
            lib_group(ui, "streams", &["take","drop","cycle","zip","by","fold","scan","size","reverse"]);
            lib_group(ui, "signals", &["sinosc","saw","pulse","wnoise","pink","combn","pluck","lpf","hpf","ar","adsr"]);
            lib_group(ui, "effects", &["verb","svflp","compressor","limiter","grain","pvocstretch"]);
            lib_group(ui, "analysis", &["goertzel","cqt","mdct","lpcanalz","fft","normalize"]);
            lib_group(ui, "i/o", &["play","stop","p","trace"]);
        });
    }

    // ── Inspector panel (graph view) ────────────────────────────────────────

    pub(crate) fn draw_inspector(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        ui.painter().line_segment(
            [rect.left_top(), rect.left_bottom()],
            Stroke::new(1.0, shell::RULE),
        );

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_width(shell::INSP_W);
            ui.add_space(14.0);
            insp_header(ui, "inspector");

            if let Some(nid) = self.selected_node {
                if let Some(node) = self.graph.node(nid) {
                    // Selected node name in warm
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(
                            egui::RichText::new(node.label())
                                .color(shell::WARM)
                                .size(13.0)
                                .monospace(),
                        );
                    });
                    ui.add_space(6.0);

                    // Key-value properties
                    insp_kv(ui, "inputs",  &format!("{}", node.inputs.len()));
                    insp_kv(ui, "outputs", &format!("{}", node.outputs.len()));
                    insp_kv(ui, "source",  if node.is_source() { "yes" } else { "no" });
                    insp_kv(ui, "sink",    if node.is_sink() { "yes" } else { "no" });
                    if let Some(adv) = &node.adverb {
                        insp_kv(ui, "adverb", &format!("{adv:?}"));
                    }

                    ui.add_space(8.0);
                    insp_header(ui, "ports");

                    for (i, port) in node.inputs.iter().enumerate() {
                        let (color, _, _) = shell::port_style(&port.kind);
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new(format!("↓{i}  {}", port.label))
                                    .color(color)
                                    .size(11.0)
                                    .monospace(),
                            );
                        });
                    }
                    for (i, port) in node.outputs.iter().enumerate() {
                        let (color, _, _) = shell::port_style(&port.kind);
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new(format!("↑{i}  {}", port.label))
                                    .color(color)
                                    .size(11.0)
                                    .monospace(),
                            );
                        });
                    }
                }
            } else {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new("click a node to inspect")
                            .color(shell::INK_3)
                            .size(11.0)
                            .monospace(),
                    );
                });
            }

            ui.add_space(12.0);
            insp_header(ui, "stack");
            crate::text::draw_stack_pub(ui, &self.interp.stack);

            ui.add_space(12.0);
            insp_header(ui, "performance");
            insp_kv(ui, "nodes", &format!("{}", self.graph.node_count()));
            insp_kv(ui, "edges", &format!("{}", self.graph.edge_count()));
            insp_kv(ui, "zoom",  &format!("{:.1}×", self.canvas_zoom));
        });
    }

    // ── Graph canvas ────────────────────────────────────────────────────────

    pub(crate) fn draw_graph_canvas(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(
            ui.available_size(),
            egui::Sense::click_and_drag(),
        );
        let rect = response.rect;
        let origin = rect.min;

        // Zoom
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.canvas_zoom = (self.canvas_zoom * (1.0 + scroll * 0.0015)).clamp(0.15, 5.0);
            }
        }

        // Pan (middle mouse or Alt+drag)
        if response.dragged_by(egui::PointerButton::Middle) {
            self.canvas_pan += response.drag_delta();
        }
        let alt = ui.input(|i| i.modifiers.alt);
        if alt && response.dragged_by(egui::PointerButton::Primary) {
            self.canvas_pan += response.drag_delta();
        }

        // Coordinate helpers
        let pan = self.canvas_pan;
        let zoom = self.canvas_zoom;
        let to_screen = |p: Pos2| -> Pos2 {
            origin + (vec2(p.x, p.y) + pan) * zoom
        };

        // Background
        painter.rect_filled(rect, 0.0, shell::PAPER);
        crate::graph::draw_dot_grid(&painter, rect, pan, zoom);

        // Compute node screen rects for hit testing
        let mut node_screen_rects: HashMap<NodeId, Rect> = HashMap::new();
        for node in self.graph.nodes_in_order() {
            let pos = self.node_positions.get(&node.id).copied().unwrap_or(pos2(20.0, 20.0));
            let sp  = to_screen(pos);
            let sz  = crate::graph::node_size(node) * zoom;
            // Include port protrusion in hit area
            let proto = shell::PORT_HALF * zoom;
            node_screen_rects.insert(
                node.id,
                Rect::from_min_size(pos2(sp.x, sp.y - proto), vec2(sz.x, sz.y + proto * 2.0)),
            );
        }

        // Interaction: drag
        let ptr = response.interact_pointer_pos();

        if response.drag_started() && !alt {
            if let Some(p) = ptr {
                for node in self.graph.nodes_in_order() {
                    if node_screen_rects.get(&node.id).is_some_and(|r| r.contains(p)) {
                        self.dragging = Some(node.id);
                        break;
                    }
                }
            }
        }

        if response.dragged() && !alt {
            if let Some(drag_id) = self.dragging {
                let canvas_delta = response.drag_delta() / zoom;
                if let Some(pos) = self.node_positions.get_mut(&drag_id) {
                    pos.x += canvas_delta.x;
                    pos.y += canvas_delta.y;
                }
            }
        }

        if !response.dragged() || alt {
            self.dragging = None;
        }

        // Interaction: click to select
        if response.clicked() {
            let mut new_sel = None;
            if let Some(p) = ptr {
                for node in self.graph.nodes_in_order() {
                    if node_screen_rects.get(&node.id).is_some_and(|r| r.contains(p)) {
                        new_sel = Some(node.id);
                        break;
                    }
                }
            }
            self.selected_node = new_sel;
        }

        // Draw wires
        for edge in self.graph.edges() {
            let src_kind = self.graph.node(edge.src.node)
                .and_then(|n| n.outputs.get(edge.src.port as usize))
                .map(|p| p.kind)
                .unwrap_or(stax_graph::PortKind::Any);

            let src_pos = self.graph.node(edge.src.node).and_then(|n| {
                let p = self.node_positions.get(&n.id).copied()?;
                Some(crate::graph::port_screen_pos(to_screen(p), n, edge.src.port, true))
            });
            let dst_pos = self.graph.node(edge.dst.node).and_then(|n| {
                let p = self.node_positions.get(&n.id).copied()?;
                Some(crate::graph::port_screen_pos(to_screen(p), n, edge.dst.port, false))
            });

            if let (Some(sp), Some(dp)) = (src_pos, dst_pos) {
                crate::graph::draw_wire(&painter, sp, dp, &src_kind, zoom);
            }
        }

        // Draw nodes
        let hover_pos = ui.input(|i| i.pointer.hover_pos());
        for node in self.graph.nodes_in_order() {
            let pos = self.node_positions.get(&node.id).copied().unwrap_or(pos2(20.0, 20.0));
            let sp  = to_screen(pos);
            let sel = self.selected_node == Some(node.id);
            let hov = hover_pos.is_some_and(|p| {
                node_screen_rects.get(&node.id).is_some_and(|r| r.contains(p))
            });
            crate::graph::draw_node(&painter, sp, node, sel, hov, zoom);
        }

        // Canvas header overlay
        crate::graph::draw_canvas_header(&painter, rect, self.view);

        // Legend
        crate::graph::draw_legend(&painter, rect);

        // Help text when graph is empty
        if self.graph.node_count() == 0 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "type in the REPL below to start\n\nexample:  440 sinosc play",
                egui::FontId::new(13.0, egui::FontFamily::Monospace),
                shell::INK_3,
            );
        }
    }
}

// ── Library helpers ────────────────────────────────────────────────────────

fn lib_header(ui: &mut egui::Ui, title: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .color(shell::INK_3)
                .size(10.0)
                .monospace(),
        );
    });
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let w = ui.available_width() - 14.0;
        let r = ui.allocate_space(vec2(w, 1.0)).1;
        ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(0.5, shell::RULE_2));
    });
    ui.add_space(8.0);
}

fn lib_group(ui: &mut egui::Ui, name: &str, words: &[&str]) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(egui::RichText::new(name).color(shell::INK).size(11.0).monospace().strong());
    });
    ui.add_space(2.0);

    // 3-column grid
    let items_per_row = 3;
    for chunk in words.chunks(items_per_row) {
        ui.horizontal(|ui| {
            ui.add_space(26.0);
            for word in chunk {
                ui.add_sized(
                    vec2(54.0, 16.0),
                    egui::Label::new(
                        egui::RichText::new(*word).color(shell::INK_2).size(11.0).monospace(),
                    ),
                );
            }
        });
    }
    ui.add_space(10.0);
}

// ── Inspector helpers ──────────────────────────────────────────────────────

fn insp_header(ui: &mut egui::Ui, title: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .color(shell::INK_3)
                .size(10.0)
                .monospace(),
        );
    });
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let w = ui.available_width() - 14.0;
        let r = ui.allocate_space(vec2(w, 1.0)).1;
        ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(0.5, shell::RULE_2));
    });
    ui.add_space(6.0);
}

fn insp_kv(ui: &mut egui::Ui, key: &str, val: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add_sized(
            vec2(64.0, 16.0),
            egui::Label::new(
                egui::RichText::new(key).color(shell::INK_2).size(11.0).monospace(),
            ),
        );
        ui.label(egui::RichText::new(val).color(shell::INK).size(11.0).monospace());
    });
}

// ── Default source shown at startup ───────────────────────────────────────

const DEFAULT_SOURCE: &str = "\
// stax — sound as pure form

440 sinosc                     // sine oscillator
2 sinosc 0.5 * 0.5 +           // AM modulator  [0.5..1.0]
*                              // amplitude modulation
play                           // send to audio output
";
