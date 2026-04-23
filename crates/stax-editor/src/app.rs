use std::collections::{HashMap, HashSet};
use egui::{pos2, vec2, Pos2, Rect, Stroke, Vec2};
use stax_core::Op;
use stax_eval::Interp;
use stax_graph::{Graph, NodeId};
use crate::fnport::FnPortState;
use crate::shell;

// ── View enum ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View { Graph, Text, FnPort, Debug }

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
    pub selected_edge: Option<stax_graph::EdgeId>,
    pub dragging: Option<NodeId>,

    // Multi-select via marquee rubber-band
    pub selected_nodes: HashSet<NodeId>,
    pub marquee_start: Option<Pos2>,   // screen-space anchor
    pub marquee_rect:  Option<Rect>,   // screen-space live rect during drag

    // Wire creation (A1)
    pub in_progress_wire: Option<stax_graph::PortRef>,
    pub wire_ghost_end: Option<Pos2>,

    // Library drag (A5)
    pub lib_drag_word: Option<String>,
    pub lib_drag_ghost: Option<Pos2>,

    // DOT viewer state
    pub show_dot_window: bool,
    pub dot_source: String,

    // Text view state
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub cursor_stack: Vec<stax_core::Value>,
    pub cursor_stack_line: usize,

    // Error position for squiggles (B1)
    pub parse_error_pos: Option<(usize, usize)>,  // (line, col) 1-indexed

    // Live column evaluation cache (B3)
    pub line_eval_cache: Vec<Option<stax_core::Value>>,
    pub line_eval_dirty: bool,

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
    // Shared ring buffer from the audio runtime (set once play starts).
    pub audio_scope: Option<std::sync::Arc<std::sync::Mutex<Vec<f32>>>>,
    // Displayable audio device stat string (e.g. "audio · 48 kHz · 128").
    pub audio_stat_str: String,

    // File management (C4)
    pub current_file: Option<std::path::PathBuf>,
    pub open_files: Vec<std::path::PathBuf>,
    // Buffer for the "open file" text input in the files panel.
    pub file_open_buf: String,
    pub file_open_active: bool,

    // Animation
    pub anim_t: f32,

    // Autocomplete (REPL + text editor)
    pub completion_candidates: Vec<String>,
    pub completion_idx: usize,
    pub show_completion: bool,

    // Pending navigation (outline/error click → jump to line)
    pub jump_to_line: Option<usize>,
    pub pending_scroll_y: Option<f32>,
    pub last_scroll_y: f32,
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
        let audio_stat_str = stax_eval::query_audio_stat();
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
            selected_edge: None,
            dragging: None,
            selected_nodes: HashSet::new(),
            marquee_start: None,
            marquee_rect: None,
            in_progress_wire: None,
            wire_ghost_end: None,
            lib_drag_word: None,
            lib_drag_ghost: None,
            show_dot_window: false,
            dot_source: String::new(),
            cursor_line: 1,
            cursor_col: 1,
            cursor_stack: Vec::new(),
            cursor_stack_line: 0,
            parse_error_pos: None,
            line_eval_cache: Vec::new(),
            line_eval_dirty: true,
            fnport: FnPortState::default(),
            rank_overrides: HashMap::new(),
            adverb_overrides: HashMap::new(),
            travel_snapshots: Vec::new(),
            travel_step: 0,
            pending_reveal: None,
            scope_samples: Vec::new(),
            audio_scope: None,
            audio_stat_str,
            current_file: None,
            open_files: Vec::new(),
            file_open_buf: String::new(),
            file_open_active: false,
            anim_t: 0.0,
            completion_candidates: Vec::new(),
            completion_idx: 0,
            show_completion: false,
            jump_to_line: None,
            pending_scroll_y: None,
            last_scroll_y: 0.0,
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
                Some("debug")  => View::Debug,
                _              => View::Graph,
            };

            // C5: restore rank/adverb overrides
            if let Some(rank_str) = s.get_string("rank_ovr") {
                for entry in rank_str.split(',').filter(|s| !s.is_empty()) {
                    let parts: Vec<&str> = entry.split(':').collect();
                    if parts.len() == 3 {
                        if let (Ok(nid_u), Ok(port_u), Ok(rank_u)) = (
                            parts[0].parse::<u32>(),
                            parts[1].parse::<u8>(),
                            parts[2].parse::<u8>(),
                        ) {
                            app.rank_overrides.insert(
                                (stax_graph::NodeId(nid_u), port_u),
                                rank_u,
                            );
                        }
                    }
                }
            }
            if let Some(adv_str) = s.get_string("adv_ovr") {
                for entry in adv_str.split(',').filter(|s| !s.is_empty()) {
                    let parts: Vec<&str> = entry.split(':').collect();
                    if parts.len() == 2 {
                        if let (Ok(nid_u), Ok(adv_u)) = (parts[0].parse::<u32>(), parts[1].parse::<u8>()) {
                            let adv = match adv_u {
                                1 => Some(stax_core::Adverb::Reduce),
                                2 => Some(stax_core::Adverb::Scan),
                                3 => Some(stax_core::Adverb::Pairwise),
                                _ => None,
                            };
                            app.adverb_overrides.insert(stax_graph::NodeId(nid_u), adv);
                        }
                    }
                }
            }

            // C4: restore current file path
            if let Some(path_str) = s.get_string("cur_file") {
                let path = std::path::PathBuf::from(path_str);
                if path.exists() {
                    app.file_open_path_inner(&path.clone());
                    app.current_file = Some(path);
                }
            }
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
            selected_edge: None,
            dragging: None,
            selected_nodes: HashSet::new(),
            marquee_start: None,
            marquee_rect: None,
            in_progress_wire: None,
            wire_ghost_end: None,
            lib_drag_word: None,
            lib_drag_ghost: None,
            show_dot_window: false,
            dot_source: String::new(),
            cursor_line: 1,
            cursor_col: 1,
            cursor_stack: Vec::new(),
            cursor_stack_line: 0,
            parse_error_pos: None,
            line_eval_cache: Vec::new(),
            line_eval_dirty: true,
            fnport: crate::fnport::FnPortState::default(),
            rank_overrides: HashMap::new(),
            adverb_overrides: HashMap::new(),
            travel_snapshots: Vec::new(),
            travel_step: 0,
            pending_reveal: None,
            scope_samples: Vec::new(),
            audio_scope: None,
            audio_stat_str: "audio".to_owned(),
            current_file: None,
            open_files: Vec::new(),
            file_open_buf: String::new(),
            file_open_active: false,
            anim_t: 0.0,
            completion_candidates: Vec::new(),
            completion_idx: 0,
            show_completion: false,
            jump_to_line: None,
            pending_scroll_y: None,
            last_scroll_y: 0.0,
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
        self.line_eval_dirty = true;
        match stax_parser::parse(&self.source) {
            Ok(ops) => {
                self.parse_error = None;
                self.parse_error_pos = None;
                self.graph = stax_graph::lift(&ops);
                let new_positions = crate::graph::auto_layout(&self.graph);
                for (id, pos) in new_positions {
                    self.node_positions.entry(id).or_insert(pos);
                }
                self.ops = ops;
            }
            Err(e) => {
                let msg = e.to_string();
                self.parse_error_pos = crate::text::extract_error_pos(&msg);
                self.parse_error = Some(msg);
            }
        }
    }

    // ── B3: compute per-line stack tops ────────────────────────────────────

    pub fn compute_line_evals(&mut self) {
        if !self.line_eval_dirty { return; }
        self.line_eval_dirty = false;
        let lines: Vec<&str> = self.source.lines().collect();
        if lines.len() > 300 {
            self.line_eval_cache = vec![None; lines.len()];
            return;
        }
        let mut results = Vec::with_capacity(lines.len());
        let mut partial = String::new();
        for line in &lines {
            partial.push_str(line);
            partial.push('\n');
            let top = stax_parser::parse(&partial).ok().and_then(|ops| {
                let mut interp = stax_eval::Interp::new();
                interp.exec(&ops).ok().and_then(|_| interp.stack.last().cloned())
            });
            results.push(top);
        }
        self.line_eval_cache = results;
    }

    // ── Persist graph mutation back to source ──────────────────────────────

    pub fn commit_graph_edit(&mut self) {
        self.source = self.graph.lower_to_source();
        self.source_modified = true;
        self.recompile();
    }

    // ── C4: File operations ────────────────────────────────────────────────

    pub fn file_new(&mut self) {
        self.source = String::new();
        self.current_file = None;
        self.source_modified = false;
        self.recompile();
    }

    /// Load source from path without updating current_file (used internally).
    fn file_open_path_inner(&mut self, path: &std::path::Path) {
        if let Ok(text) = std::fs::read_to_string(path) {
            self.source = text;
            self.source_modified = false;
        }
    }

    pub fn file_open_path(&mut self, path: std::path::PathBuf) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            self.source = text;
            if !self.open_files.contains(&path) {
                self.open_files.push(path.clone());
            }
            self.current_file = Some(path);
            self.source_modified = false;
            self.recompile();
        }
    }

    pub fn file_save(&mut self) {
        if let Some(ref path) = self.current_file.clone() {
            if let Err(e) = std::fs::write(path, &self.source) {
                eprintln!("stax: save failed: {e}");
            } else {
                self.source_modified = false;
            }
        }
    }

    pub fn file_save_as(&mut self, path: std::path::PathBuf) {
        if !self.open_files.contains(&path) {
            self.open_files.push(path.clone());
        }
        self.current_file = Some(path);
        self.file_save();
    }

    // ── C2/C3: Scope ring buffer drain ─────────────────────────────────────

    pub fn update_audio_scope(&mut self) {
        // Acquire scope ring once audio runtime starts.
        if self.audio_scope.is_none() {
            self.audio_scope = self.interp.scope_ring();
        }
        // Update the audio stat string once we have live data.
        if let Some((sr, buf)) = self.interp.audio_stat() {
            self.audio_stat_str = format!("audio · {} kHz · {}", sr / 1000, buf);
        }
        // Drain new samples into the scope buffer.
        if let Some(ref ring) = self.audio_scope.clone() {
            if let Ok(mut guard) = ring.try_lock() {
                self.scope_samples.extend(guard.drain(..));
                let keep = 256usize;
                if self.scope_samples.len() > keep {
                    let excess = self.scope_samples.len() - keep;
                    self.scope_samples.drain(0..excess);
                }
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

impl StaxApp {
    /// Render a complete UI frame into `ctx` without needing `eframe::Frame`.
    /// Used by headed tests so they can drive the full app shell via egui_kittest.
    pub fn render_frame(&mut self, ctx: &egui::Context) {
        self.anim_t = ctx.input(|i| i.time as f32);

        // Consume pending reveal
        if let Some(target) = self.pending_reveal.take() {
            match target {
                RevealTarget::GraphNode(nid) => {
                    self.view = View::Graph;
                    self.selected_node = Some(nid);
                    if let Some(&pos) = self.node_positions.get(&nid) {
                        self.canvas_pan = -vec2(pos.x - 200.0, pos.y - 200.0);
                    }
                }
                RevealTarget::TextLine(line) => {
                    self.view = View::Text;
                    self.cursor_line = line;
                    self.cursor_stack_line = 0;
                }
            }
        }

        let frame_none = egui::Frame::new()
            .fill(shell::PAPER)
            .inner_margin(egui::Margin::ZERO);

        egui::TopBottomPanel::top("rf_header")
            .exact_height(shell::HEADER_H).frame(frame_none.fill(shell::PAPER))
            .show_separator_line(false).show(ctx, |ui| self.draw_header(ui));

        egui::TopBottomPanel::top("rf_tabs")
            .exact_height(shell::TABS_H).frame(frame_none.fill(shell::PAPER_2))
            .show_separator_line(false).show(ctx, |ui| self.draw_tabs(ui));

        egui::TopBottomPanel::bottom("rf_botbar")
            .exact_height(shell::BOTBAR_H).frame(frame_none.fill(shell::PAPER))
            .show_separator_line(false).show(ctx, |ui| self.draw_botbar(ui));

        if matches!(self.view, View::Graph | View::FnPort | View::Debug) {
            egui::TopBottomPanel::bottom("rf_repl")
                .exact_height(shell::REPL_H).frame(frame_none)
                .show_separator_line(false).show(ctx, |ui| self.draw_graph_repl(ui));
            egui::TopBottomPanel::bottom("rf_timebar")
                .exact_height(shell::TIMEBAR_H).frame(frame_none)
                .show_separator_line(false).show(ctx, |ui| self.draw_timebar(ui));
        }

        // DOT viewer window (shown as a floating overlay)
        if self.show_dot_window {
            crate::dot::draw_dot_window(ctx, &mut self.show_dot_window, &mut self.dot_source, &self.graph);
        }

        match self.view {
            View::Graph => {
                egui::SidePanel::left("rf_lib").exact_width(shell::LIB_W).frame(frame_none)
                    .show_separator_line(false).show(ctx, |ui| self.draw_library(ui));
                egui::SidePanel::right("rf_insp").exact_width(shell::INSP_W).frame(frame_none)
                    .show_separator_line(false).show(ctx, |ui| self.draw_inspector(ui));
                egui::CentralPanel::default().frame(frame_none)
                    .show(ctx, |ui| self.draw_graph_canvas(ui));
            }
            View::FnPort => {
                egui::SidePanel::left("rf_lib").exact_width(shell::LIB_W).frame(frame_none)
                    .show_separator_line(false).show(ctx, |ui| self.draw_library(ui));
                egui::SidePanel::right("rf_insp").exact_width(shell::INSP_W).frame(frame_none)
                    .show_separator_line(false).show(ctx, |ui| self.draw_inspector(ui));
                egui::CentralPanel::default().frame(frame_none)
                    .show(ctx, |ui| self.draw_fnport_view(ui));
            }
            View::Text => {
                egui::SidePanel::left("rf_files").exact_width(shell::LIB_W).frame(frame_none)
                    .show_separator_line(false).show(ctx, |ui| self.draw_files_panel(ui));
                egui::SidePanel::right("rf_side").exact_width(shell::SIDE_W).frame(frame_none)
                    .show_separator_line(false).show(ctx, |ui| self.draw_text_side(ui));
                egui::CentralPanel::default().frame(frame_none)
                    .show(ctx, |ui| self.draw_text_editor(ui));
            }
            View::Debug => {
                egui::CentralPanel::default().frame(frame_none)
                    .show(ctx, |ui| self.draw_debug_view(ui));
            }
        }
    }
}

impl eframe::App for StaxApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("cpx", self.canvas_pan.x.to_string());
        storage.set_string("cpy", self.canvas_pan.y.to_string());
        storage.set_string("czm", self.canvas_zoom.to_string());
        let view_str = match self.view {
            View::Graph  => "graph",
            View::Text   => "text",
            View::FnPort => "fnport",
            View::Debug  => "debug",
        };
        storage.set_string("view", view_str.to_owned());

        // C5: persist rank / adverb overrides
        let rank_str: String = self.rank_overrides.iter()
            .map(|((nid, port), rank)| format!("{}:{}:{}", nid.0, port, rank))
            .collect::<Vec<_>>().join(",");
        storage.set_string("rank_ovr", rank_str);

        let adv_str: String = self.adverb_overrides.iter()
            .map(|(nid, adv)| {
                let code: u8 = match adv {
                    None => 0,
                    Some(stax_core::Adverb::Reduce)   => 1,
                    Some(stax_core::Adverb::Scan)     => 2,
                    Some(stax_core::Adverb::Pairwise) => 3,
                };
                format!("{}:{}", nid.0, code)
            })
            .collect::<Vec<_>>().join(",");
        storage.set_string("adv_ovr", adv_str);

        // C4: persist current file path
        if let Some(ref path) = self.current_file {
            storage.set_string("cur_file", path.to_string_lossy().to_string());
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.anim_t = ctx.input(|i| i.time as f32);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        self.update_audio_scope();

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
        if matches!(self.view, View::Graph | View::FnPort | View::Debug) {
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

        // DOT viewer window
        if self.show_dot_window {
            crate::dot::draw_dot_window(ctx, &mut self.show_dot_window, &mut self.dot_source, &self.graph);
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
            View::Debug => {
                egui::CentralPanel::default()
                    .frame(frame_none)
                    .show(ctx, |ui| self.draw_debug_view(ui));
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
                    self.file_new();
                    ui.close_menu();
                }
                if ui.button("open...").clicked() {
                    self.file_open_active = true;
                    self.view = View::Text;
                    ui.close_menu();
                }
                if ui.button("save").clicked() {
                    self.file_save();
                    ui.close_menu();
                }
                if ui.button("save as...").clicked() {
                    self.file_open_active = true;
                    self.view = View::Text;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("revert to default").clicked() {
                    self.source = DEFAULT_SOURCE.to_owned();
                    self.current_file = None;
                    self.recompile();
                    ui.close_menu();
                }
            });
            ui.add_space(4.0);

            // ── View menu ─────────────────────────────────────────────────
            egui::menu::menu_button(ui, egui::RichText::new("view").color(shell::INK_2).size(12.0).monospace(), |ui| {
                if ui.button("graph").clicked()   { self.view = View::Graph;  ui.close_menu(); }
                if ui.button("text").clicked()    { self.view = View::Text;   ui.close_menu(); }
                if ui.button("fn-port").clicked() { self.view = View::FnPort; ui.close_menu(); }
                if ui.button("debug").clicked()   { self.view = View::Debug;  ui.close_menu(); }
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
                ui.separator();
                // DOT viewer
                if ui.button("show DOT…").clicked() {
                    self.dot_source = crate::dot::graph_to_dot(&self.graph);
                    self.show_dot_window = true;
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

                // Audio stat (real device info once queried)
                let blink = (self.anim_t * 0.667).fract() < 0.5;
                let dot = if blink { "●" } else { "○" };
                ui.label(
                    egui::RichText::new(format!("{dot}  {}", self.audio_stat_str))
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
                (View::Debug,  "debug"),
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
                ("✕  parse error", shell::ERR)
            } else {
                ("✓  ready", shell::COOL)
            };
            ui.label(
                egui::RichText::new(status.0)
                    .color(status.1)
                    .size(11.0)
                    .monospace(),
            );

            // B5: cursor position + stats in text view
            if matches!(self.view, View::Text) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(14.0);
                    ui.label(egui::RichText::new(
                        format!("L{}:{} · utf-8 · {} nodes",
                            self.cursor_line, self.cursor_col, self.graph.node_count())
                    ).color(shell::INK_3).size(10.0).monospace());
                });
            }
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
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        match entry.kind {
                            ReplKind::Input => {
                                ui.label(egui::RichText::new("›  ").color(shell::INK).size(12.0).monospace());
                                let mut job = crate::syntax::layout_job_sized(&entry.text, 12.0);
                                job.wrap.max_width = f32::INFINITY;
                                ui.label(egui::widget_text::WidgetText::LayoutJob(job));
                            }
                            ReplKind::Output => { ui.label(egui::RichText::new(format!("   {}", entry.text)).color(shell::INK_2).size(12.0).monospace()); }
                            ReplKind::Ok     => { ui.label(egui::RichText::new(format!("   {}", entry.text)).color(shell::COOL).size(12.0).monospace()); }
                            ReplKind::Err    => { ui.label(egui::RichText::new(format!("   {}", entry.text)).color(shell::WARM).size(12.0).monospace()); }
                        }
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
                bar_painter.rect_filled(fill, 0.0, shell::RULE_2);

                // Playhead: 1.5px WARM vertical line (spec: "border: WARM")
                let head_x = track.min.x + fill_w;
                bar_painter.line_segment(
                    [pos2(head_x, br.min.y + 2.0), pos2(head_x, br.max.y - 2.0)],
                    Stroke::new(1.5, shell::WARM),
                );

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

        // Update ghost position while dragging (A5)
        if self.lib_drag_word.is_some() {
            self.lib_drag_ghost = ui.input(|i| i.pointer.hover_pos());
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_width(shell::LIB_W);
            ui.add_space(14.0);

            lib_header(ui, "library");

            let mut started: Option<String> = None;
            lib_group(ui, "math", &["+","-","×","÷","%","pow","sqrt","abs","neg","to","ord","nat"], &mut started);
            lib_group(ui, "streams", &["take","drop","cycle","zip","by","fold","scan","size","reverse"], &mut started);
            lib_group(ui, "signals", &["sinosc","saw","pulse","wnoise","pink","combn","pluck","lpf","hpf","ar","adsr"], &mut started);
            lib_group(ui, "effects", &["verb","svflp","compressor","limiter","grain","pvocstretch"], &mut started);
            lib_group(ui, "analysis", &["goertzel","cqt","mdct","lpcanalz","fft","normalize"], &mut started);
            lib_group(ui, "i/o", &["play","stop","p","trace"], &mut started);

            if let Some(word) = started {
                self.lib_drag_word = Some(word);
                self.lib_drag_ghost = ui.input(|i| i.pointer.hover_pos());
            }
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

        // Zoom — centered on cursor
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let old_zoom = self.canvas_zoom;
                let new_zoom = (old_zoom * (1.0 + scroll * 0.0015)).clamp(0.15, 5.0);
                if let Some(cursor) = ui.input(|i| i.pointer.hover_pos()) {
                    self.canvas_pan += (cursor - origin) * (1.0 / new_zoom - 1.0 / old_zoom);
                }
                self.canvas_zoom = new_zoom;
            }
        }

        // Pan (middle mouse or Alt+drag)
        let alt = ui.input(|i| i.modifiers.alt);
        if response.dragged_by(egui::PointerButton::Middle) {
            self.canvas_pan += response.drag_delta() / self.canvas_zoom;
        }
        if alt && response.dragged_by(egui::PointerButton::Primary) {
            self.canvas_pan += response.drag_delta() / self.canvas_zoom;
        }

        let pan  = self.canvas_pan;
        let zoom = self.canvas_zoom;
        let to_screen = |p: Pos2| -> Pos2 { origin + (vec2(p.x, p.y) + pan) * zoom };

        // Background
        painter.rect_filled(rect, 0.0, shell::PAPER);
        crate::graph::draw_dot_grid(&painter, rect, pan, zoom);

        // Build node screen rects for hit-testing
        let mut node_screen_rects: HashMap<NodeId, Rect> = HashMap::new();
        for node in self.graph.nodes_in_order() {
            let pos = self.node_positions.get(&node.id).copied().unwrap_or(pos2(20.0, 20.0));
            let sp  = to_screen(pos);
            let sz  = crate::graph::node_size(node) * zoom;
            let proto = shell::PORT_HALF * zoom;
            node_screen_rects.insert(
                node.id,
                Rect::from_min_size(pos2(sp.x, sp.y - proto), vec2(sz.x, sz.y + proto * 2.0)),
            );
        }

        let hover_pos = ui.input(|i| i.pointer.hover_pos());
        let ptr       = response.interact_pointer_pos();

        // Which node is currently hovered? Used by context menu.
        let hovered_node: Option<NodeId> = hover_pos.and_then(|p| {
            self.graph.nodes_in_order()
                .find(|n| node_screen_rects.get(&n.id).is_some_and(|r| r.contains(p)))
                .map(|n| n.id)
        });

        // Which output port is hovered? Used to start wire drags.
        let hovered_output: Option<(NodeId, u8)> = hover_pos.and_then(|hp| {
            self.graph.nodes_in_order().find_map(|n| {
                let sp = self.node_positions.get(&n.id).map(|&p| to_screen(p))?;
                crate::graph::port_at_screen(hp, n, sp, zoom, true).map(|pi| (n.id, pi))
            })
        });

        // ── Delete key: remove selected node(s) or edge ─────────────────────
        let delete_pressed = ui.input(|i| i.key_pressed(egui::Key::Delete)
            || i.key_pressed(egui::Key::Backspace));
        if delete_pressed {
            if let Some(eid) = self.selected_edge.take() {
                self.graph.remove_edge(eid);
                self.commit_graph_edit();
            } else if !self.selected_nodes.is_empty() {
                let to_delete: Vec<NodeId> = self.selected_nodes.drain().collect();
                for nid in &to_delete {
                    self.graph.remove_node(*nid);
                    self.node_positions.remove(nid);
                }
                if let Some(sel) = self.selected_node {
                    if to_delete.contains(&sel) { self.selected_node = None; }
                }
                self.commit_graph_edit();
            } else if let Some(nid) = self.selected_node.take() {
                self.graph.remove_node(nid);
                self.node_positions.remove(&nid);
                self.commit_graph_edit();
            }
        }

        // ── drag_started: wire creation, node drag, or marquee ──────────────
        if response.drag_started() && !alt {
            if let Some(p) = ptr {
                if let Some((nid, port_idx)) = hovered_output {
                    // Start a wire from this output port
                    self.in_progress_wire = Some(stax_graph::PortRef { node: nid, port: port_idx });
                    self.wire_ghost_end = Some(p);
                    self.dragging = None;
                } else {
                    // Start node drag or marquee on empty space
                    self.in_progress_wire = None;
                    let hit = self.graph.nodes_in_order()
                        .find(|n| node_screen_rects.get(&n.id).is_some_and(|r| r.contains(p)))
                        .map(|n| n.id);
                    self.dragging = hit;
                    // Begin marquee when dragging over empty space
                    if hit.is_none() {
                        self.marquee_start = Some(p);
                        self.marquee_rect  = None;
                    }
                }
            }
        }

        // ── Primary drag: wire ghost, node move, or marquee update ──────────
        if response.dragged_by(egui::PointerButton::Primary) && !alt {
            if self.in_progress_wire.is_some() {
                if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                    self.wire_ghost_end = Some(p);
                }
            } else if let Some(drag_id) = self.dragging {
                let canvas_delta = response.drag_delta() / zoom;
                if let Some(pos) = self.node_positions.get_mut(&drag_id) {
                    pos.x += canvas_delta.x;
                    pos.y += canvas_delta.y;
                }
            } else if let Some(anchor) = self.marquee_start {
                // Update marquee rect
                if let Some(cur) = ui.input(|i| i.pointer.interact_pos()) {
                    self.marquee_rect = Some(Rect::from_two_pos(anchor, cur));
                }
            } else {
                self.canvas_pan += response.drag_delta() / self.canvas_zoom;
            }
        }

        // ── drag released: finish wire, marquee select, or clear drag ────────
        if response.drag_stopped() {
            if let Some(src_ref) = self.in_progress_wire.take() {
                if let Some(drop_p) = self.wire_ghost_end.take() {
                    let dst = self.graph.nodes_in_order().find_map(|n| {
                        let sp = self.node_positions.get(&n.id).map(|&p| to_screen(p))?;
                        crate::graph::port_at_screen(drop_p, n, sp, zoom, false)
                            .map(|pi| stax_graph::PortRef { node: n.id, port: pi })
                    });
                    if let Some(dst_ref) = dst {
                        if self.graph.add_edge(src_ref, dst_ref).is_some() {
                            self.commit_graph_edit();
                        }
                    }
                }
            }
            // Collect nodes inside marquee
            if let Some(mrect) = self.marquee_rect.take() {
                self.selected_nodes.clear();
                for node in self.graph.nodes_in_order() {
                    if let Some(sr) = node_screen_rects.get(&node.id) {
                        if mrect.intersects(*sr) {
                            self.selected_nodes.insert(node.id);
                        }
                    }
                }
            }
            self.marquee_start = None;
            self.wire_ghost_end = None;
            if !response.dragged() { self.dragging = None; }
        }
        if !response.dragged() { self.dragging = None; }

        // ── Click: select node or edge (Shift+click toggles marquee set) ─────
        if response.clicked() {
            if let Some(p) = ptr {
                let shift = ui.input(|i| i.modifiers.shift);
                let hit_node = self.graph.nodes_in_order()
                    .find(|n| node_screen_rects.get(&n.id).is_some_and(|r| r.contains(p)))
                    .map(|n| n.id);
                if let Some(nid) = hit_node {
                    if shift {
                        // Shift+click toggles node in multi-select set
                        if self.selected_nodes.contains(&nid) {
                            self.selected_nodes.remove(&nid);
                        } else {
                            self.selected_nodes.insert(nid);
                        }
                    } else {
                        self.selected_node = Some(nid);
                        self.selected_edge = None;
                        self.selected_nodes.clear();
                        self.fnport.selected_node = None; // sync fnport tab to new selection
                    }
                } else {
                    // Try to select edge
                    let mut hit_edge = None;
                    'edge_search: for edge in self.graph.edges() {
                        let sp = self.graph.node(edge.src.node).and_then(|n| {
                            let pos = self.node_positions.get(&n.id).copied()?;
                            Some(crate::graph::port_screen_pos(to_screen(pos), n, edge.src.port, true))
                        });
                        let dp = self.graph.node(edge.dst.node).and_then(|n| {
                            let pos = self.node_positions.get(&n.id).copied()?;
                            Some(crate::graph::port_screen_pos(to_screen(pos), n, edge.dst.port, false))
                        });
                        if let (Some(sp), Some(dp)) = (sp, dp) {
                            if crate::graph::bezier_hit_test(sp, dp, p, zoom) {
                                hit_edge = Some(edge.id);
                                break 'edge_search;
                            }
                        }
                    }
                    if hit_edge.is_some() {
                        self.selected_edge = hit_edge;
                        self.selected_node = None;
                    } else {
                        self.selected_node = None;
                        self.selected_edge = None;
                        if !shift { self.selected_nodes.clear(); }
                    }
                }
            }
        }

        // ── Draw wires (two passes: normal, then selected in WARM) ──────────
        let sel_eid = self.selected_edge;
        for edge in self.graph.edges() {
            if Some(edge.id) == sel_eid { continue; } // draw selected last
            let src_kind = self.graph.node(edge.src.node)
                .and_then(|n| n.outputs.get(edge.src.port as usize))
                .map(|p| p.kind)
                .unwrap_or(stax_graph::PortKind::Any);
            let sp = self.graph.node(edge.src.node).and_then(|n| {
                let p = self.node_positions.get(&n.id).copied()?;
                Some(crate::graph::port_screen_pos(to_screen(p), n, edge.src.port, true))
            });
            let dp = self.graph.node(edge.dst.node).and_then(|n| {
                let p = self.node_positions.get(&n.id).copied()?;
                Some(crate::graph::port_screen_pos(to_screen(p), n, edge.dst.port, false))
            });
            if let (Some(sp), Some(dp)) = (sp, dp) {
                crate::graph::draw_wire(&painter, sp, dp, &src_kind, zoom);
            }
        }
        // Draw selected edge in WARM / 2px
        if let Some(eid) = sel_eid {
            if let Some(edge) = self.graph.edges().iter().find(|e| e.id == eid) {
                let sp = self.graph.node(edge.src.node).and_then(|n| {
                    let p = self.node_positions.get(&n.id).copied()?;
                    Some(crate::graph::port_screen_pos(to_screen(p), n, edge.src.port, true))
                });
                let dp = self.graph.node(edge.dst.node).and_then(|n| {
                    let p = self.node_positions.get(&n.id).copied()?;
                    Some(crate::graph::port_screen_pos(to_screen(p), n, edge.dst.port, false))
                });
                if let (Some(sp), Some(dp)) = (sp, dp) {
                    let pts: Vec<Pos2> = {
                        let dy = ((dp.y - sp.y).abs() * 0.5).max(40.0 * zoom);
                        let c1 = pos2(sp.x, sp.y + dy);
                        let c2 = pos2(dp.x, dp.y - dy);
                        (0..=24).map(|i| {
                            let t = i as f32 / 24.0; let u = 1.0 - t;
                            pos2(u*u*u*sp.x+3.0*u*u*t*c1.x+3.0*u*t*t*c2.x+t*t*t*dp.x,
                                 u*u*u*sp.y+3.0*u*u*t*c1.y+3.0*u*t*t*c2.y+t*t*t*dp.y)
                        }).collect()
                    };
                    painter.add(egui::Shape::line(pts, Stroke::new(2.0 * zoom.sqrt(), shell::WARM)));
                }
            }
        }

        // ── Draw ghost wire (A1 in-progress) ───────────────────────────────
        if let (Some(src_ref), Some(ghost_end)) = (self.in_progress_wire, self.wire_ghost_end) {
            let src_screen = self.graph.node(src_ref.node).and_then(|n| {
                let p = self.node_positions.get(&n.id).copied()?;
                Some(crate::graph::port_screen_pos(to_screen(p), n, src_ref.port, true))
            });
            if let Some(ss) = src_screen {
                crate::graph::draw_wire_ghost(&painter, ss, ghost_end, zoom);
            }
        }

        // ── Draw nodes ──────────────────────────────────────────────────────
        let click_pos = if response.clicked() { ptr } else { None };
        let mut interact_zones: Vec<(stax_graph::NodeId, Vec<crate::graph::NodeInteract>)> = Vec::new();

        for node in self.graph.nodes_in_order() {
            let pos = self.node_positions.get(&node.id).copied().unwrap_or(pos2(20.0, 20.0));
            let sp  = to_screen(pos);
            let sel = self.selected_node == Some(node.id) || self.selected_nodes.contains(&node.id);
            let hov = hover_pos.is_some_and(|p| {
                node_screen_rects.get(&node.id).is_some_and(|r| r.contains(p))
            });

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
                &port_ranks, adverb_override, scope,
            );
            interact_zones.push((node.id, zones));
        }

        // Handle badge clicks (rank/adverb cycle)
        if let Some(cp) = click_pos {
            'outer: for (nid, zones) in &interact_zones {
                for iz in zones {
                    if iz.zone.contains(cp) {
                        match iz.action {
                            crate::graph::NodeAction::CyclePortRank(port_idx) => {
                                let current = self.rank_overrides.get(&(*nid, port_idx)).copied().unwrap_or(0);
                                let next = (current + 1) % 5;
                                self.rank_overrides.insert((*nid, port_idx), next);
                            }
                            crate::graph::NodeAction::CycleAdverb => {
                                use stax_core::Adverb;
                                let current = self.adverb_overrides.get(nid).copied().flatten();
                                let next = match current {
                                    None                   => Some(Adverb::Reduce),
                                    Some(Adverb::Reduce)   => Some(Adverb::Scan),
                                    Some(Adverb::Scan)     => Some(Adverb::Pairwise),
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

        // ── Context menu: D2 port ops + D3 wire ops + D5 info + A3/A4 ────────
        // Pre-compute hovered input port and hovered edge for context menu use.
        // Use selected node as fallback so right-click works even after mouse moves off.
        let ctx_hovered_node  = hovered_node.or(self.selected_node);
        let ctx_hovered_input: Option<(NodeId, u8, stax_graph::PortKind)> =
            hover_pos.and_then(|hp| {
                self.graph.nodes_in_order().find_map(|n| {
                    let sp = self.node_positions.get(&n.id).map(|&p| to_screen(p))?;
                    crate::graph::port_at_screen(hp, n, sp, zoom, false).map(|pi| {
                        let kind = n.inputs.get(pi as usize)
                            .map(|p| p.kind)
                            .unwrap_or(stax_graph::PortKind::Any);
                        (n.id, pi, kind)
                    })
                })
            });
        let ctx_hovered_edge: Option<stax_graph::EdgeId> =
            if ctx_hovered_node.is_none() {
                hover_pos.and_then(|hp| {
                    self.graph.edges().iter().find_map(|edge| {
                        let sp = self.graph.node(edge.src.node).and_then(|n| {
                            let p = self.node_positions.get(&n.id).copied()?;
                            Some(crate::graph::port_screen_pos(to_screen(p), n, edge.src.port, true))
                        });
                        let dp = self.graph.node(edge.dst.node).and_then(|n| {
                            let p = self.node_positions.get(&n.id).copied()?;
                            Some(crate::graph::port_screen_pos(to_screen(p), n, edge.dst.port, false))
                        });
                        match (sp, dp) {
                            (Some(s), Some(d)) if crate::graph::bezier_hit_test(s, d, hp, zoom) => Some(edge.id),
                            _ => None,
                        }
                    })
                })
            } else {
                None
            };

        response.context_menu(|ui| {
            if let Some(nid) = ctx_hovered_node {
                // Function info header
                if let Some(node) = self.graph.node(nid) {
                    let label = node.label();
                    if let Some(desc) = crate::graph::word_description(&label) {
                        ui.label(egui::RichText::new(desc).color(shell::INK_2).size(10.0).monospace());
                    }
                    let arity_str = crate::graph::node_arity_string(node);
                    let type_str  = crate::graph::node_port_type_string(node);
                    if !arity_str.is_empty() {
                        ui.label(egui::RichText::new(arity_str).color(shell::INK_3).size(9.0).monospace());
                    }
                    if !type_str.is_empty() {
                        ui.label(egui::RichText::new(type_str).color(shell::INK_3).size(9.0).monospace());
                    }
                    ui.separator();
                }

                if ui.button("Delete node").clicked() {
                    ui.close_menu();
                    self.graph.remove_node(nid);
                    self.node_positions.remove(&nid);
                    if self.selected_node == Some(nid) { self.selected_node = None; }
                    self.selected_nodes.remove(&nid);
                    self.commit_graph_edit();
                }
                if !self.selected_nodes.is_empty()
                    && ui.button(format!("Delete {} selected", self.selected_nodes.len())).clicked() {
                    ui.close_menu();
                    let to_del: Vec<NodeId> = self.selected_nodes.drain().collect();
                    for did in &to_del {
                        self.graph.remove_node(*did);
                        self.node_positions.remove(did);
                        if self.selected_node == Some(*did) { self.selected_node = None; }
                    }
                    self.commit_graph_edit();
                }
                if ui.button("Disconnect all").clicked() {
                    ui.close_menu();
                    let eids = self.graph.edges_of(nid);
                    for eid in eids { self.graph.remove_edge(eid); }
                    self.commit_graph_edit();
                }

                // Port-specific ops when a Fun input is hovered
                if let Some((port_nid, port_idx, kind)) = ctx_hovered_input {
                    if port_nid == nid {
                        ui.separator();
                        if kind == stax_graph::PortKind::Fun
                            && ui.button("View fn body").clicked() {
                            ui.close_menu();
                            self.fnport.selected_node = Some(nid);
                            self.view = crate::app::View::FnPort;
                        }
                        if ui.button(format!("Disconnect port in:{port_idx}")).clicked() {
                            ui.close_menu();
                            let eids: Vec<_> = self.graph.edges().iter()
                                .filter(|e| e.dst.node == nid && e.dst.port == port_idx)
                                .map(|e| e.id)
                                .collect();
                            for eid in eids { self.graph.remove_edge(eid); }
                            self.commit_graph_edit();
                        }
                    }
                }

                ui.separator();
            }

            // Wire delete when hovering edge but no node
            if ctx_hovered_node.is_none() {
                if let Some(eid) = ctx_hovered_edge {
                    if ui.button("Delete wire").clicked() {
                        ui.close_menu();
                        self.graph.remove_edge(eid);
                        if self.selected_edge == Some(eid) { self.selected_edge = None; }
                        self.commit_graph_edit();
                    }
                    ui.separator();
                }
            }

            // A4: add-node submenu
            let click_canvas_pos = ui.input(|i| i.pointer.interact_pos())
                .map(|p| crate::graph::screen_to_canvas(p, pan, zoom, origin));

            ui.menu_button("Add node …", |ui| {
                let groups: &[(&str, &[&str])] = &[
                    ("math",    &["+", "-", "*", "/", "pow", "sqrt", "abs", "neg", "%"]),
                    ("compare", &["<", ">", "==", "min", "max", "clip"]),
                    ("streams", &["ord", "nat", "by", "cyc", "N", "take", "drop", "zip"]),
                    ("signals", &["sinosc", "saw", "pulse", "wnoise", "pink", "lpf", "hpf", "svflp"]),
                    ("envelope",&["ar", "adsr", "decay", "line", "xline"]),
                    ("effects", &["verb", "pan2", "compressor", "grain", "pluck"]),
                    ("i/o",     &["play", "stop", "p", "trace", "normalize"]),
                ];
                for (group_name, words) in groups {
                    ui.label(egui::RichText::new(*group_name).color(shell::INK_3).size(9.0).monospace());
                    for word in words.iter() {
                        if ui.small_button(*word).clicked() {
                            let world_pos = click_canvas_pos.unwrap_or(pos2(100.0, 100.0));
                            let nid = self.graph.add_word_node(word);
                            self.node_positions.insert(nid, world_pos);
                            self.commit_graph_edit();
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                }
            });
        });

        // ── Library drag ghost (A5) ─────────────────────────────────────────
        if let Some(ref word) = self.lib_drag_word.clone() {
            if let Some(ghost_screen) = self.lib_drag_ghost {
                if rect.contains(ghost_screen) {
                    let ghost_rect = Rect::from_center_size(
                        ghost_screen,
                        vec2(shell::NODE_MIN_W * zoom, shell::NODE_HDR_H * zoom),
                    );
                    painter.rect_stroke(ghost_rect, 0.0,
                        Stroke::new(1.0, shell::PORT_FUN), egui::StrokeKind::Outside);
                    painter.text(ghost_rect.center(), egui::Align2::CENTER_CENTER,
                        word.as_str(),
                        egui::FontId::new(11.0 * zoom.sqrt(), egui::FontFamily::Monospace),
                        shell::PORT_FUN);
                }
            }
            // On pointer release: drop onto canvas (D4: drop on Fun port → quote node)
            if !ui.input(|i| i.pointer.any_down()) {
                if let Some(ghost_screen) = self.lib_drag_ghost {
                    if rect.contains(ghost_screen) {
                        // Check if dropping onto a Fun input port
                        let fun_port_target: Option<(NodeId, u8)> =
                            self.graph.nodes_in_order().find_map(|n| {
                                let sp = self.node_positions.get(&n.id).map(|&p| to_screen(p))?;
                                let pi = crate::graph::port_at_screen(ghost_screen, n, sp, zoom, false)?;
                                let kind = n.inputs.get(pi as usize).map(|p| p.kind)?;
                                if kind == stax_graph::PortKind::Fun {
                                    Some((n.id, pi))
                                } else {
                                    None
                                }
                            });

                        let world_pos = crate::graph::screen_to_canvas(ghost_screen, pan, zoom, origin);
                        if let Some((dst_nid, dst_port)) = fun_port_target {
                            // Create a quote node [word] and connect it to the Fun port
                            let quote_nid = self.graph.add_word_node(&format!("[{}]", word));
                            let offset_pos = pos2(world_pos.x - 120.0, world_pos.y);
                            self.node_positions.insert(quote_nid, offset_pos);
                            let src_ref = stax_graph::PortRef { node: quote_nid, port: 0 };
                            let dst_ref = stax_graph::PortRef { node: dst_nid,   port: dst_port };
                            let _ = self.graph.add_edge(src_ref, dst_ref);
                        } else {
                            let nid = self.graph.add_word_node(word);
                            self.node_positions.insert(nid, world_pos);
                        }
                        self.commit_graph_edit();
                    }
                }
                self.lib_drag_word = None;
                self.lib_drag_ghost = None;
            }
        }

        // Draw marquee rect
        if let Some(mrect) = self.marquee_rect {
            let fill = egui::Color32::from_rgba_premultiplied(201, 72, 32, 20);
            painter.rect_filled(mrect, 0.0, fill);
            painter.rect_stroke(mrect, 0.0, Stroke::new(1.0, shell::WARM), egui::StrokeKind::Outside);
        }

        // Legend
        crate::graph::draw_legend(&painter, rect);

        // Help text when graph is empty
        if self.graph.node_count() == 0 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "type in the REPL below to start\n\nexample:  440 0 sinosc play",
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

fn lib_group(ui: &mut egui::Ui, name: &str, words: &[&str], drag_started: &mut Option<String>) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(egui::RichText::new(name).color(shell::INK).size(11.0).monospace().strong());
    });
    ui.add_space(2.0);

    let items_per_row = 3;
    for chunk in words.chunks(items_per_row) {
        ui.horizontal(|ui| {
            ui.add_space(26.0);
            for word in chunk {
                let resp = ui.add_sized(
                    vec2(54.0, 16.0),
                    egui::Label::new(
                        egui::RichText::new(*word).color(shell::INK_2).size(11.0).monospace(),
                    ).sense(egui::Sense::click_and_drag()),
                );
                if resp.drag_started() && drag_started.is_none() {
                    *drag_started = Some(word.to_string());
                }
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

440 0 sinosc                   // sine oscillator
2 0 sinosc 0.5 * 0.5 +         // AM modulator  [0.5..1.0]
*                              // amplitude modulation
play                           // send to audio output
";
