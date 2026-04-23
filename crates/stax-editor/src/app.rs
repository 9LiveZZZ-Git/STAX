use std::collections::HashMap;
use egui::{pos2, vec2, Pos2, Rect, Stroke, Vec2};
use stax_core::Op;
use stax_eval::Interp;
use stax_graph::{Graph, NodeId};
use crate::fnport::FnPortState;
use crate::shell;

// ── View enum ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View { Graph, Text, FnPort }

// ── Time-travel snapshot ───────────────────────────────────────────────────

/// Stack state captured after a successful REPL execution step.
pub struct TravelSnapshot {
    pub stack: Vec<stax_core::Value>,
    pub label: String,
}

// ── Reveal router ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum RevealTarget {
    GraphNode(stax_graph::NodeId),
    TextLine(usize),
}

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
    pub cursor_stack: Vec<stax_core::Value>,
    pub cursor_stack_line: usize,

    // Fn-port view state
    pub fnport: FnPortState,

    // Rank / adverb overrides (interactive badges in graph view)
    pub rank_overrides: HashMap<(NodeId, u8), u8>,  // (node_id, port_idx) → rank code 0–4
    pub adverb_overrides: HashMap<NodeId, Option<stax_core::Adverb>>,

    // Time-travel
    pub travel_snapshots: Vec<TravelSnapshot>,
    pub travel_step: usize,

    // Reveal router (queued cross-view jump, consumed on next frame)
    pub pending_reveal: Option<RevealTarget>,

    // Scope samples for sink-node waveforms (last N audio output samples)
    pub scope_samples: Vec<f32>,

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
            cursor_stack: Vec::new(),
            cursor_stack_line: 0,
            fnport: FnPortState::default(),
            rank_overrides: HashMap::new(),
            adverb_overrides: HashMap::new(),
            travel_snapshots: Vec::new(),
            travel_step: 0,
            pending_reveal: None,
            scope_samples: Vec::new(),
            anim_t: 0.0,
            source,
        };

        // Load saved layout from eframe persistent storage
        if let Some(s) = cc.storage {
            let parse_f32 = |key: &str, default: f32| -> f32 {
                s.get_string(key).and_then(|v| v.parse().ok()).unwrap_or(default)
            };
            app.canvas_pan.x = parse_f32("cpx", 0.0);
            app.canvas_pan.y = parse_f32("cpy", 0.0);
            app.canvas_zoom  = parse_f32("czm", 1.0).clamp(0.15, 5.0);
            app.view = match s.get_string("view").as_deref() {
                Some("text")   => View::Text,
                Some("fnport") => View::FnPort,
                _              => View::Graph,
            };
        }

        app.recompile();
        app
    }

    // ── Test constructor (no eframe::CreationContext needed) ───────────────

    pub fn new_for_test() -> Self {
        let source = DEFAULT_SOURCE.to_owned();
        let mut app = Self {
            view: View::Graph,
            source_modified: false,
            ops: Vec::new(),
            graph: stax_graph::Graph::new(),
            parse_error: None,
            interp: stax_eval::Interp::new(),
            repl_input: String::new(),
            repl_history: Vec::new(),
            canvas_pan: Vec2::ZERO,
            canvas_zoom: 1.0,
            node_positions: HashMap::new(),
            selected_node: None,
            dragging: None,
            cursor_line: 1,
            cursor_stack: Vec::new(),
            cursor_stack_line: 0,
            fnport: crate::fnport::FnPortState::default(),
            rank_overrides: HashMap::new(),
            adverb_overrides: HashMap::new(),
            travel_snapshots: Vec::new(),
            travel_step: 0,
            pending_reveal: None,
            scope_samples: Vec::new(),
            anim_t: 0.0,
            source,
        };
        app.recompile();
        app
    }

    // ── Cursor-stack: re-eval lines 1..cursor_line ─────────────────────────

    pub fn compute_cursor_stack(&mut self) {
        if self.cursor_line == self.cursor_stack_line { return; }
        let partial: String = self.source.lines()
            .take(self.cursor_line)
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(ops) = stax_parser::parse(&partial) {
            let mut interp = stax_eval::Interp::new();
            let _ = interp.exec(&ops);
            self.cursor_stack = interp.stack;
        } else {
            self.cursor_stack = Vec::new();
        }
        self.cursor_stack_line = self.cursor_line;
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
                        // Record time-travel snapshot
                        self.travel_snapshots.push(TravelSnapshot {
                            stack: self.interp.stack.clone(),
                            label: line.to_owned(),
                        });
                        if self.travel_snapshots.len() > 1000 {
                            self.travel_snapshots.drain(0..100);
                        }
                        self.travel_step = self.travel_snapshots.len().saturating_sub(1);
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
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("cpx", self.canvas_pan.x.to_string());
        storage.set_string("cpy", self.canvas_pan.y.to_string());
        storage.set_string("czm", self.canvas_zoom.to_string());
        let view_str = match self.view {
            View::Graph  => "graph",
            View::Text   => "text",
            View::FnPort => "fnport",
        };
        storage.set_string("view", view_str.to_owned());
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.anim_t = ctx.input(|i| i.time as f32);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        // ── Reveal router: consume pending cross-view jump ─────────────────
        if let Some(target) = self.pending_reveal.take() {
            match target {
                RevealTarget::GraphNode(nid) => {
                    self.view = View::Graph;
                    self.selected_node = Some(nid);
                    // Pan canvas so the node is visible
                    if let Some(&pos) = self.node_positions.get(&nid) {
                        self.canvas_pan = -vec2(pos.x - 200.0, pos.y - 200.0);
                    }
                }
                RevealTarget::TextLine(line) => {
                    self.view = View::Text;
                    self.cursor_line = line;
                    self.cursor_stack_line = 0; // force recompute
                }
            }
        }

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
            View::Graph => {
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
            View::FnPort => {
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
                    .show(ctx, |ui| self.draw_fnport_view(ui));
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
    fn draw_header(&mut self, ui: &mut egui::Ui) {
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
            ui.add_space(8.0);

            // ── File menu ─────────────────────────────────────────────────
            egui::menu::menu_button(ui, egui::RichText::new("file").color(shell::INK_2).size(12.0).monospace(), |ui| {
                if ui.button("new patch").clicked() {
                    self.source = String::new();
                    self.recompile();
                    ui.close_menu();
                }
                if ui.button("revert to default").clicked() {
                    self.source = DEFAULT_SOURCE.to_owned();
                    self.recompile();
                    ui.close_menu();
                }
            });
            ui.add_space(4.0);

            // ── View menu ─────────────────────────────────────────────────
            egui::menu::menu_button(ui, egui::RichText::new("view").color(shell::INK_2).size(12.0).monospace(), |ui| {
                if ui.button("graph").clicked() { self.view = View::Graph; ui.close_menu(); }
                if ui.button("text").clicked()  { self.view = View::Text; ui.close_menu(); }
                if ui.button("fn-port").clicked() { self.view = View::FnPort; ui.close_menu(); }
                ui.separator();
                if ui.button("reset canvas").clicked() {
                    self.canvas_pan = Vec2::ZERO;
                    self.canvas_zoom = 1.0;
                    ui.close_menu();
                }
                if ui.button("fit to nodes").clicked() {
                    self.fit_canvas_to_nodes();
                    ui.close_menu();
                }
            });
            ui.add_space(4.0);

            // ── Run menu ──────────────────────────────────────────────────
            egui::menu::menu_button(ui, egui::RichText::new("run").color(shell::INK_2).size(12.0).monospace(), |ui| {
                if ui.button("compile (⌘R)").clicked() {
                    self.recompile();
                    ui.close_menu();
                }
                if ui.button("play").clicked() {
                    self.exec_repl("play");
                    ui.close_menu();
                }
                if ui.button("stop").clicked() {
                    self.exec_repl("stop");
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("clear stack").clicked() {
                    self.interp.stack.clear();
                    ui.close_menu();
                }
            });
            ui.add_space(4.0);

            // ── Help menu ─────────────────────────────────────────────────
            egui::menu::menu_button(ui, egui::RichText::new("help").color(shell::INK_2).size(12.0).monospace(), |ui| {
                ui.label(egui::RichText::new("stax — sound as pure form").color(shell::INK_2).size(11.0).monospace());
                ui.separator();
                ui.label(egui::RichText::new("REPL commands:").color(shell::INK_3).size(10.0).monospace());
                ui.label(egui::RichText::new(".s  show stack").color(shell::INK_2).size(10.0).monospace());
                ui.label(egui::RichText::new(".c  clear stack").color(shell::INK_2).size(10.0).monospace());
            });

            // Right-aligned: audio status + transport
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);

                // Transport: stop
                let sep = ui.allocate_space(vec2(1.0, 20.0)).1;
                ui.painter().line_segment(
                    [sep.center_top(), sep.center_bottom()],
                    Stroke::new(1.0, shell::RULE),
                );
                ui.add_space(8.0);
                let stop_r = ui.add(egui::Label::new(
                    egui::RichText::new("■ stop").color(shell::WARM).size(12.0).monospace()
                ).sense(egui::Sense::click()));
                if stop_r.clicked() { self.exec_repl("stop"); }
                ui.add_space(8.0);

                // Transport: play
                let play_r = ui.add(egui::Label::new(
                    egui::RichText::new("▶ play").color(shell::INK).size(12.0).monospace()
                ).sense(egui::Sense::click()));
                if play_r.clicked() { self.exec_repl("play"); }
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

    pub fn fit_canvas_to_nodes(&mut self) {
        if self.node_positions.is_empty() { return; }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for &pos in self.node_positions.values() {
            min_x = min_x.min(pos.x);
            min_y = min_y.min(pos.y);
            max_x = max_x.max(pos.x + 120.0);
            max_y = max_y.max(pos.y + 40.0);
        }
        let w = max_x - min_x;
        let h = max_y - min_y;
        let pad = 60.0_f32;
        // Center at 600×400 viewport estimate
        self.canvas_zoom = ((600.0 / (w + pad * 2.0)).min(400.0 / (h + pad * 2.0))).clamp(0.2, 2.0);
        self.canvas_pan = vec2(
            -(min_x - pad) + (600.0 / self.canvas_zoom - w) * 0.5 - pad,
            -(min_y - pad) + (400.0 / self.canvas_zoom - h) * 0.5 - pad,
        );
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

    fn draw_timebar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        ui.painter().line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, shell::RULE),
        );

        let total = self.travel_snapshots.len();
        let step  = self.travel_step;

        ui.horizontal(|ui| {
            ui.set_min_height(shell::TIMEBAR_H);
            ui.add_space(14.0);

            // ◀◀ — jump to step 0
            let r = ui.add(egui::Label::new(
                egui::RichText::new("◀◀").color(shell::INK).size(13.0).monospace()
            ).sense(egui::Sense::click()));
            if r.clicked() && total > 0 { self.travel_step = 0; }
            ui.add_space(4.0);

            // ◀ — step back
            let r = ui.add(egui::Label::new(
                egui::RichText::new("◀").color(shell::INK).size(13.0).monospace()
            ).sense(egui::Sense::click()));
            if r.clicked() && step > 0 { self.travel_step -= 1; }
            ui.add_space(4.0);

            // ❚❚ — jump to latest
            let r = ui.add(egui::Label::new(
                egui::RichText::new("❚❚").color(shell::INK).size(13.0).monospace()
            ).sense(egui::Sense::click()));
            if r.clicked() && total > 0 { self.travel_step = total.saturating_sub(1); }
            ui.add_space(4.0);

            // ▶ — step forward
            let r = ui.add(egui::Label::new(
                egui::RichText::new("▶").color(shell::INK).size(13.0).monospace()
            ).sense(egui::Sense::click()));
            if r.clicked() && step + 1 < total { self.travel_step += 1; }
            ui.add_space(4.0);

            // ▶▶ — jump to end
            let r = ui.add(egui::Label::new(
                egui::RichText::new("▶▶").color(shell::INK).size(13.0).monospace()
            ).sense(egui::Sense::click()));
            if r.clicked() && total > 0 { self.travel_step = total.saturating_sub(1); }
            ui.add_space(8.0);

            let step_label = if total == 0 {
                "step — / —".to_owned()
            } else {
                format!("step {} / {}", step + 1, total)
            };
            ui.label(
                egui::RichText::new(step_label).color(shell::INK_2).size(11.0).monospace(),
            );

            // Scrub bar — drag to scrub through snapshots
            ui.add_space(8.0);
            let bar_w = (ui.available_width() - 14.0).max(0.0);
            let (bar_resp, bar_painter) = ui.allocate_painter(vec2(bar_w, shell::TIMEBAR_H - 8.0), egui::Sense::drag());
            let br = bar_resp.rect;
            let track = Rect::from_center_size(br.center(), vec2(br.width(), 6.0));
            bar_painter.rect_filled(track, 0.0, shell::SURFACE);
            bar_painter.rect_stroke(track, 0.0, Stroke::new(1.0, shell::RULE_2), egui::StrokeKind::Outside);

            if total > 1 {
                // Fill up to current step
                let fill_w = track.width() * (step as f32 / (total - 1) as f32);
                let fill = Rect::from_min_size(track.min, vec2(fill_w, track.height()));
                bar_painter.rect_filled(fill, 0.0, shell::INK_2);

                // Draggable thumb
                let thumb_x = track.min.x + fill_w;
                let thumb = Rect::from_center_size(pos2(thumb_x, track.center().y), vec2(6.0, 12.0));
                bar_painter.rect_filled(thumb, 0.0, shell::INK);

                if bar_resp.dragged() {
                    if let Some(pos) = bar_resp.interact_pointer_pos() {
                        let t = ((pos.x - track.min.x) / track.width()).clamp(0.0, 1.0);
                        self.travel_step = ((t * (total - 1) as f32).round() as usize).min(total - 1);
                    }
                }
            }
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

        // Zoom — centered on cursor so the point under the pointer stays fixed
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let old_zoom = self.canvas_zoom;
                let new_zoom = (old_zoom * (1.0 + scroll * 0.0015)).clamp(0.15, 5.0);
                // Adjust pan so the canvas point under the cursor is invariant:
                //   to_screen(p) = origin + (p + pan) * zoom
                //   new_pan = old_pan + (cursor - origin) * (1/new_zoom - 1/old_zoom)
                if let Some(cursor) = ui.input(|i| i.pointer.hover_pos()) {
                    self.canvas_pan += (cursor - origin) * (1.0 / new_zoom - 1.0 / old_zoom);
                }
                self.canvas_zoom = new_zoom;
            }
        }

        // Pan (middle mouse or Alt+drag) — divide by zoom: drag_delta is screen px, pan is canvas units
        let alt = ui.input(|i| i.modifiers.alt);
        if response.dragged_by(egui::PointerButton::Middle) {
            self.canvas_pan += response.drag_delta() / self.canvas_zoom;
        }
        if alt && response.dragged_by(egui::PointerButton::Primary) {
            self.canvas_pan += response.drag_delta() / self.canvas_zoom;
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
                let mut hit_node = false;
                for node in self.graph.nodes_in_order() {
                    if node_screen_rects.get(&node.id).is_some_and(|r| r.contains(p)) {
                        self.dragging = Some(node.id);
                        hit_node = true;
                        break;
                    }
                }
                if !hit_node {
                    // Empty-space drag = pan
                    self.dragging = None;
                }
            }
        }

        if response.dragged_by(egui::PointerButton::Primary) && !alt {
            if let Some(drag_id) = self.dragging {
                // Drag node
                let canvas_delta = response.drag_delta() / zoom;
                if let Some(pos) = self.node_positions.get_mut(&drag_id) {
                    pos.x += canvas_delta.x;
                    pos.y += canvas_delta.y;
                }
            } else {
                // Drag empty space → pan canvas
                self.canvas_pan += response.drag_delta() / self.canvas_zoom;
            }
        }

        if !response.dragged() {
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

        // Draw nodes and collect badge interactions
        let hover_pos = ui.input(|i| i.pointer.hover_pos());
        let click_pos = if response.clicked() { ptr } else { None };
        let mut interact_zones: Vec<(stax_graph::NodeId, Vec<crate::graph::NodeInteract>)> = Vec::new();

        for node in self.graph.nodes_in_order() {
            let pos = self.node_positions.get(&node.id).copied().unwrap_or(pos2(20.0, 20.0));
            let sp  = to_screen(pos);
            let sel = self.selected_node == Some(node.id);
            let hov = hover_pos.is_some_and(|p| {
                node_screen_rects.get(&node.id).is_some_and(|r| r.contains(p))
            });

            // Build per-port rank array from overrides
            let n_inputs = node.inputs.len();
            let port_ranks: Vec<u8> = (0..n_inputs as u8)
                .map(|i| self.rank_overrides.get(&(node.id, i)).copied().unwrap_or(0))
                .collect();

            let adverb_override = self.adverb_overrides.get(&node.id).copied().flatten();

            let scope = if node.is_sink() && !self.scope_samples.is_empty() {
                Some(self.scope_samples.as_slice())
            } else {
                None
            };

            let (_body, zones) = crate::graph::draw_node(
                &painter, sp, node, sel, hov, zoom,
                &port_ranks,
                adverb_override,
                scope,
            );
            interact_zones.push((node.id, zones));
        }

        // Handle badge clicks (rank cycle / adverb cycle)
        // These must come after draw so painter is no longer borrowed
        if let Some(cp) = click_pos {
            'outer: for (nid, zones) in &interact_zones {
                for iz in zones {
                    if iz.zone.contains(cp) {
                        match iz.action {
                            crate::graph::NodeAction::CyclePortRank(port_idx) => {
                                let current = self.rank_overrides.get(&(*nid, port_idx)).copied().unwrap_or(0);
                                let next = (current + 1) % 5; // 0=none,1=@,2=@1,3=@2,4=@@
                                self.rank_overrides.insert((*nid, port_idx), next);
                            }
                            crate::graph::NodeAction::CycleAdverb => {
                                use stax_core::Adverb;
                                let current = self.adverb_overrides.get(nid).copied().flatten();
                                let next = match current {
                                    None                  => Some(Adverb::Reduce),
                                    Some(Adverb::Reduce)  => Some(Adverb::Scan),
                                    Some(Adverb::Scan)    => Some(Adverb::Pairwise),
                                    Some(Adverb::Pairwise) => None,
                                };
                                self.adverb_overrides.insert(*nid, next);
                            }
                        }
                        break 'outer;
                    }
                }
            }
        }

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
