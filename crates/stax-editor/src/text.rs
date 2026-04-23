use egui::{pos2, vec2, Color32, Id, Rect, Stroke};
use stax_core::Value;
use crate::{app::StaxApp, shell};

// ── Builtin hover-doc table ────────────────────────────────────────────────
// Each entry: (name, signature, description)
static BUILTIN_DOCS: &[(&str, &str, &str)] = &[
    ("+",       "a b → c",                    "Add two numbers or zip-add two streams"),
    ("-",       "a b → c",                    "Subtract b from a"),
    ("*",       "a b → c",                    "Multiply; can modulate signals"),
    ("/",       "a b → c",                    "Divide a by b"),
    ("sinosc",  "freq → Signal",              "Sine oscillator at given frequency (Hz)"),
    ("saw",     "freq → Signal",              "Bandlimited sawtooth oscillator"),
    ("play",    "Signal →",                   "Send signal to audio output and start playback"),
    ("ord",     "n → Stream",                 "Stream of integers 1..n (inclusive)"),
    ("nat",     "→ Stream",                   "Infinite stream of natural numbers 1, 2, 3, …"),
    ("N",       "stream n → stream",          "Take first n elements from a stream"),
    ("cyc",     "stream → Stream",            "Cycle a finite stream infinitely"),
    ("dup",     "a → a a",                    "Duplicate top of stack"),
    ("drop",    "a →",                        "Drop top of stack"),
    ("swap",    "a b → b a",                  "Swap top two stack items"),
    ("ar",      "atk rel → Env",              "Attack-release envelope (linear)"),
    ("adsr",    "a d s r → Env",              "Attack-decay-sustain-release envelope"),
    ("lpf",     "sig freq → Signal",          "2-pole lowpass filter (Butterworth)"),
    ("hpf",     "sig freq → Signal",          "2-pole highpass filter (Butterworth)"),
    ("verb",    "sig room damp → Signal",     "FDN reverb (Jot/Hadamard topology)"),
    ("pan2",    "sig pos → [L R]",            "Pan mono signal to stereo; pos in [-1, 1]"),
];

// ── Did-you-mean helpers ───────────────────────────────────────────────────

static KNOWN_WORDS: &[&str] = &[
    "+", "-", "*", "/", "dup", "drop", "swap", "over",
    "sinosc", "saw", "pulse", "wnoise", "pink", "lpf", "hpf", "svflp",
    "play", "stop", "ar", "adsr", "verb", "pan2", "pluck",
    "ord", "nat", "cyc", "by", "N", "to", "take", "drop",
    "lorenz", "rossler", "goertzel", "cqt", "lpcanalz",
    "p", "trace", "normalize",
];

fn word_similarity(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn best_suggestion(unknown: &str) -> Option<&'static str> {
    if unknown.len() < 2 { return None; }
    let best = KNOWN_WORDS.iter()
        .map(|&w| (w, word_similarity(unknown, w)))
        .max_by_key(|&(_, s)| s)?;
    if best.1 >= 2 { Some(best.0) } else { None }
}

fn extract_unknown_word(err: &str) -> Option<&str> {
    // Look for text between single or double quotes first
    for delim in &['\'', '"'] {
        if let Some(start) = err.find(*delim) {
            let rest = &err[start + 1..];
            if let Some(end) = rest.find(*delim) {
                let word = &rest[..end];
                if !word.is_empty() {
                    return Some(word);
                }
            }
        }
    }
    // Fall back to last space-separated token
    err.split_whitespace().last()
}

impl StaxApp {
    // ── Left panel: files + outline + diagnostics ──────────────────────────

    pub(crate) fn draw_files_panel(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        ui.painter().line_segment(
            [rect.right_top(), rect.right_bottom()],
            Stroke::new(1.0, shell::RULE),
        );

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_width(shell::LIB_W);
            ui.add_space(6.0);

            // ── FILES ──
            section_header(ui, "project", None);
            ui.add_space(4.0);

            let is_active = true;
            file_row(ui, "patch.stax", is_active);
            file_row(ui, "prelude.stax", false);

            ui.add_space(8.0);

            // ── OUTLINE ──
            let bindings = self.outline_bindings();
            section_header(ui, "outline", Some(format!("{} ↓", bindings.len())));
            ui.add_space(4.0);

            for (name, line) in &bindings {
                outline_row(ui, name, *line, false);
            }

            // ── DIAGNOSTICS ──
            ui.add_space(8.0);
            let err_label = if self.parse_error.is_some() {
                Some("1 err".to_owned())
            } else {
                None
            };
            section_header(ui, "diagnostics", err_label);
            ui.add_space(4.0);

            if let Some(err) = &self.parse_error {
                let msg = err.clone();
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.label(
                        egui::RichText::new(format!("✕  {msg}"))
                            .color(shell::WARM)
                            .size(11.0)
                            .monospace(),
                    );
                });

                // Did-you-mean suggestion
                let suggestion = extract_unknown_word(&msg)
                    .and_then(|w| best_suggestion(w));
                if let Some(s) = suggestion {
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        ui.label(
                            egui::RichText::new(format!("did you mean: {s}"))
                                .color(shell::INK_2)
                                .size(11.0)
                                .monospace(),
                        );
                    });
                }
            } else {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.label(
                        egui::RichText::new("no issues")
                            .color(shell::INK_3)
                            .size(11.0)
                            .monospace(),
                    );
                });
            }

            ui.add_space(12.0);
        });
    }

    // ── Centre: editable code editor with syntax highlighting ─────────────

    pub(crate) fn draw_text_editor(&mut self, ui: &mut egui::Ui) {
        const GUTTER_W: f32 = 32.0;
        const ROW_H: f32 = 18.0;

        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);

        // ── Breadcrumb bar ─────────────────────────────────────────────────
        let bc_h = 24.0;
        let bc_rect = Rect::from_min_size(rect.min, vec2(rect.width(), bc_h));
        ui.painter().rect_filled(bc_rect, 0.0, shell::PAPER_2);
        ui.painter().line_segment(
            [bc_rect.left_bottom(), bc_rect.right_bottom()],
            Stroke::new(0.5, shell::RULE_2),
        );

        // 3px WARM left border on breadcrumb when there is a parse error
        if self.parse_error.is_some() {
            ui.painter().line_segment(
                [bc_rect.left_top(), bc_rect.left_bottom()],
                Stroke::new(3.0, shell::WARM),
            );
        }

        ui.painter().text(
            pos2(bc_rect.min.x + 14.0, bc_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "patch.stax",
            egui::FontId::new(11.0, egui::FontFamily::Monospace),
            shell::INK_2,
        );
        if self.source_modified {
            ui.painter().text(
                pos2(bc_rect.max.x - 14.0, bc_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "● modified",
                egui::FontId::new(10.0, egui::FontFamily::Monospace),
                shell::WARM,
            );
        }

        // ── Code area (gutter + editor) ────────────────────────────────────
        let full_code_rect = Rect::from_min_size(
            pos2(rect.min.x, rect.min.y + bc_h),
            vec2(rect.width(), rect.height() - bc_h),
        );

        // Gutter rect (left 32px)
        let gutter_rect = Rect::from_min_size(
            full_code_rect.min,
            vec2(GUTTER_W, full_code_rect.height()),
        );
        // Editor rect (remainder)
        let editor_rect = Rect::from_min_max(
            pos2(full_code_rect.min.x + GUTTER_W + 1.0, full_code_rect.min.y),
            full_code_rect.max,
        );

        // Draw gutter background and right border
        ui.painter().rect_filled(gutter_rect, 0.0, shell::PAPER_2);
        ui.painter().line_segment(
            [gutter_rect.right_top(), gutter_rect.right_bottom()],
            Stroke::new(1.0, shell::RULE),
        );

        // ── Editor scroll area ─────────────────────────────────────────────
        let mut code_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(editor_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );

        let mut layouter = |ui: &egui::Ui, s: &str, _wrap_width: f32| -> std::sync::Arc<egui::Galley> {
            let mut job = crate::syntax::layout_job(s);
            job.wrap.max_width = f32::INFINITY;
            ui.fonts(|f| f.layout_job(job))
        };

        let te_id = Id::new("stax_text_editor");

        let scroll_out = egui::ScrollArea::both()
            .id_salt("code_scroll")
            .show(&mut code_ui, |ui| {
                let te = egui::TextEdit::multiline(&mut self.source)
                    .id(te_id)
                    .font(egui::FontId::new(13.0, egui::FontFamily::Monospace))
                    .text_color(shell::INK)
                    .frame(false)
                    .desired_rows(40)
                    .desired_width(f32::INFINITY)
                    .code_editor()
                    .layouter(&mut layouter);

                let out = te.show(ui);

                // Track cursor line from cursor_range byte offset
                if let Some(cursor_range) = out.cursor_range {
                    let byte_offset = cursor_range.primary.ccursor.index;
                    let line = self.source[..byte_offset.min(self.source.len())]
                        .chars()
                        .filter(|&c| c == '\n')
                        .count()
                        + 1;
                    self.cursor_line = line;
                }

                // On any edit: recompile and track modification
                if out.response.changed() {
                    self.source_modified = true;
                    self.recompile();
                    if self.parse_error.is_none() {
                        self.source_modified = false;
                    }
                }

                out.response
            });

        let scroll_y = scroll_out.state.offset.y;
        let te_resp = scroll_out.inner;

        // ── Gutter line numbers ────────────────────────────────────────────
        let total_lines = self.source.lines().count().max(1);
        let visible_start = (scroll_y / ROW_H) as usize;
        let visible_end = visible_start + (gutter_rect.height() / ROW_H) as usize + 2;
        let visible_end = visible_end.min(total_lines);

        let gutter_y = gutter_rect.min.y;
        let scroll_frac = scroll_y.rem_euclid(ROW_H);

        for line in (visible_start + 1)..=(visible_end + 1) {
            if line > total_lines { break; }
            let row_top = gutter_y + (line - 1 - visible_start) as f32 * ROW_H - scroll_frac;
            let row_rect = Rect::from_min_size(
                pos2(gutter_rect.min.x, row_top),
                vec2(GUTTER_W, ROW_H),
            );

            // Active line highlight
            if line == self.cursor_line {
                ui.painter().rect_filled(row_rect, 0.0, shell::PAPER);
                ui.painter().line_segment(
                    [row_rect.left_top(), row_rect.left_bottom()],
                    Stroke::new(2.0, shell::WARM),
                );
            }

            // Line number text
            ui.painter().text(
                pos2(gutter_rect.max.x - 6.0, row_top + ROW_H * 0.5),
                egui::Align2::RIGHT_CENTER,
                line.to_string(),
                egui::FontId::new(10.0, egui::FontFamily::Monospace),
                shell::INK_3,
            );
        }

        // ── Error underline ────────────────────────────────────────────────
        if self.parse_error.is_some() {
            let r = te_resp.rect;
            ui.painter().line_segment(
                [r.left_bottom(), r.right_bottom()],
                Stroke::new(1.0, shell::WARM),
            );
        }

        // ── Hover-doc tooltip ──────────────────────────────────────────────
        let ctx = ui.ctx().clone();
        if let Some(hover_pos) = ctx.input(|i| i.pointer.hover_pos()) {
            if full_code_rect.contains(hover_pos) {
                if let Some(word) = word_at_screen_pos(&self.source, hover_pos, editor_rect) {
                    if let Some(doc) = lookup_doc(&word) {
                        let tooltip_id = Id::new("stax_hover_doc");
                        let layer_id = egui::LayerId::new(egui::Order::Tooltip, tooltip_id);
                        egui::show_tooltip_at_pointer(&ctx, layer_id, tooltip_id, |ui| {
                            ui.label(
                                egui::RichText::new(doc.0)
                                    .color(shell::INK)
                                    .size(12.0)
                                    .monospace()
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(doc.1)
                                    .color(shell::PORT_FUN)
                                    .size(11.0)
                                    .monospace(),
                            );
                            ui.label(
                                egui::RichText::new(doc.2)
                                    .color(shell::INK_2)
                                    .size(11.0)
                                    .monospace(),
                            );
                        });
                    }
                }
            }
        }
    }

    // ── Right panel: stack + inspector + REPL ─────────────────────────────

    pub(crate) fn draw_text_side(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);
        ui.painter().line_segment(
            [rect.left_top(), rect.left_bottom()],
            Stroke::new(1.0, shell::RULE),
        );

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_width(shell::SIDE_W);

            // ── STACK AT CURSOR ──
            if self.cursor_stack.is_empty() && self.cursor_stack_line == 0 {
                section_header(ui, "stack", None);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new("(run REPL to see stack)")
                            .color(shell::INK_3)
                            .size(11.0)
                            .monospace(),
                    );
                });
            } else {
                section_header(ui, &format!("stack at line {}", self.cursor_stack_line), None);
                draw_stack_contents(ui, &self.cursor_stack);
            }

            ui.add_space(4.0);
            ui.separator();

            // ── INSPECTOR ──
            section_header(ui, "inspector", None);
            if let Some(nid) = self.selected_node {
                if let Some(node) = self.graph.node(nid) {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(
                            egui::RichText::new(node.label())
                                .color(shell::WARM)
                                .size(13.0)
                                .monospace(),
                        );
                    });
                    kv_row(ui, "kind",  &format!("{:?}", node.kind));
                    kv_row(ui, "in",   &format!("{}", node.inputs.len()));
                    kv_row(ui, "out",  &format!("{}", node.outputs.len()));
                    if let Some(adv) = &node.adverb {
                        kv_row(ui, "adverb", &format!("{adv:?}"));
                    }
                }
            } else {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new("nothing selected")
                            .color(shell::INK_3)
                            .size(11.0)
                            .monospace(),
                    );
                });
            }

            ui.add_space(4.0);
            ui.separator();

            // ── REPL ──
            self.draw_repl_panel(ui);
        });
    }

    // ── Helper: REPL panel (shared by graph + text side panels) ───────────

    pub(crate) fn draw_repl_panel(&mut self, ui: &mut egui::Ui) {
        section_header(ui, "repl", None);

        // History (scrollable, capped to last 200 lines)
        let history_h = 140.0;
        egui::ScrollArea::vertical()
            .id_salt("repl_hist")
            .max_height(history_h)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let history = self.repl_history.clone();
                for entry in &history {
                    use crate::app::ReplKind::*;
                    let (prefix, color) = match entry.kind {
                        Input  => ("›  ", shell::INK),
                        Output => ("   ", shell::INK_2),
                        Ok     => ("   ", shell::COOL),
                        Err    => ("   ", shell::WARM),
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

        // Input field
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("›  ").color(shell::INK_3).size(12.0).monospace(),
            );
            let resp = ui.add(
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
        });
        ui.add_space(4.0);
    }

    // ── Outline helper ─────────────────────────────────────────────────────

    fn outline_bindings(&self) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        for (line_idx, line) in self.source.lines().enumerate() {
            let trimmed = line.trim();
            if let Some(pos) = trimmed.rfind(" = ") {
                let name = &trimmed[pos + 3..];
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    out.push((name.to_owned(), line_idx + 1));
                }
            }
        }
        out
    }
}

// ── Hover-doc helpers ──────────────────────────────────────────────────────

fn lookup_doc(word: &str) -> Option<(&'static str, &'static str, &'static str)> {
    BUILTIN_DOCS
        .iter()
        .find(|(name, _, _)| *name == word)
        .map(|&(n, s, d)| (n, s, d))
}

fn word_at_screen_pos(source: &str, hover: egui::Pos2, code_rect: Rect) -> Option<String> {
    const CHAR_W: f32 = 7.8;
    const ROW_H: f32 = 18.0;

    let rel_x = (hover.x - code_rect.min.x).max(0.0);
    let rel_y = (hover.y - code_rect.min.y).max(0.0);

    let row = (rel_y / ROW_H) as usize;
    let col = (rel_x / CHAR_W) as usize;

    let line = source.lines().nth(row)?;

    let mut byte_idx = 0usize;
    for (i, c) in line.char_indices() {
        if i >= col {
            byte_idx = i;
            break;
        }
        byte_idx = i + c.len_utf8();
    }
    byte_idx = byte_idx.min(line.len());

    let before = &line[..byte_idx];
    let token_start = before
        .rfind(|c: char| !is_word_char(c) && !is_op_char(c))
        .map(|p| p + 1)
        .unwrap_or(0);

    let after = &line[byte_idx..];
    let token_end_rel = after
        .find(|c: char| !is_word_char(c) && !is_op_char(c))
        .unwrap_or(after.len());
    let token_end = byte_idx + token_end_rel;

    let token = &line[token_start..token_end];
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_op_char(c: char) -> bool {
    matches!(
        c,
        '+' | '-' | '*' | '/' | '!' | '%' | '@' | '&' | '|' | '^' | '~' | '#'
    )
}

// ── Free-standing panel helpers ────────────────────────────────────────────

fn section_header(ui: &mut egui::Ui, title: &str, meta: Option<String>) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .color(shell::INK_3)
                .size(10.0)
                .monospace(),
        );
        if let Some(m) = meta {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(m).color(shell::INK_3).size(9.0).monospace(),
                );
            });
        }
    });
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let lw = ui.available_width() - 14.0;
        let rect = ui.allocate_space(vec2(lw, 1.0)).1;
        ui.painter().line_segment(
            [rect.left_center(), rect.right_center()],
            Stroke::new(0.5, shell::RULE_2),
        );
    });
    ui.add_space(4.0);
}

fn file_row(ui: &mut egui::Ui, name: &str, active: bool) {
    ui.horizontal(|ui| {
        if active {
            let rect = ui.max_rect();
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, vec2(rect.width(), 18.0)),
                0.0,
                shell::SURFACE,
            );
            ui.painter().line_segment(
                [rect.min, pos2(rect.min.x, rect.min.y + 18.0)],
                Stroke::new(2.0, shell::WARM),
            );
        }
        ui.add_space(if active { 16.0 } else { 18.0 });
        ui.label(
            egui::RichText::new(name)
                .color(if active { shell::INK } else { shell::INK_2 })
                .size(11.0)
                .monospace(),
        );
    });
}

fn outline_row(ui: &mut egui::Ui, name: &str, line: usize, active: bool) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        ui.label(
            egui::RichText::new(name)
                .color(if active { shell::INK } else { shell::INK_2 })
                .size(11.0)
                .monospace(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(line.to_string())
                    .color(shell::INK_3)
                    .size(9.0)
                    .monospace(),
            );
        });
    });
}

fn kv_row(ui: &mut egui::Ui, key: &str, val: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add_sized(
            vec2(64.0, 16.0),
            egui::Label::new(
                egui::RichText::new(key).color(shell::INK_2).size(11.0).monospace(),
            ),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(val).color(shell::INK).size(11.0).monospace(),
        );
    });
}

pub fn draw_stack_pub(ui: &mut egui::Ui, stack: &[Value]) {
    draw_stack_contents(ui, stack);
}

fn draw_stack_contents(ui: &mut egui::Ui, stack: &[Value]) {
    if stack.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("(empty)")
                    .color(shell::INK_3)
                    .size(11.0)
                    .monospace(),
            );
        });
        return;
    }
    for (i, val) in stack.iter().rev().enumerate().take(12) {
        let idx  = format!("{i}");
        let kind = value_kind_label(val);
        let repr = format_value(val);
        let (kind_color, _) = value_kind_color(val);

        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.add_sized(
                vec2(24.0, 16.0),
                egui::Label::new(
                    egui::RichText::new(idx).color(shell::INK_3).size(11.0).monospace(),
                ),
            );
            ui.add_sized(
                vec2(52.0, 16.0),
                egui::Label::new(
                    egui::RichText::new(kind).color(kind_color).size(10.0).monospace(),
                ),
            );
            ui.label(
                egui::RichText::new(repr).color(shell::INK).size(10.0).monospace(),
            );
        });
    }
    if stack.len() > 12 {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(format!("… {} more", stack.len() - 12))
                    .color(shell::INK_3)
                    .size(10.0)
                    .monospace(),
            );
        });
    }
}

fn value_kind_label(v: &Value) -> &'static str {
    match v {
        Value::Real(_)   => "real",
        Value::Str(_)    => "str",
        Value::Sym(_)    => "sym",
        Value::Stream(_) => "stream",
        Value::Signal(_) => "signal",
        Value::Form(_)   => "form",
        Value::Fun(_)    => "fun",
        Value::Ref(_)    => "ref",
        Value::Nil       => "nil",
    }
}

fn value_kind_color(v: &Value) -> (Color32, bool) {
    match v {
        Value::Real(_)   => (shell::PORT_REAL,   false),
        Value::Signal(_) => (shell::PORT_SIGNAL, false),
        Value::Stream(_) => (shell::PORT_STREAM, true),
        Value::Fun(_)    => (shell::PORT_FUN,    false),
        Value::Form(_)   => (shell::PORT_FORM,   false),
        _                => (shell::INK_2,       false),
    }
}

pub fn format_value_pub(v: &Value) -> String {
    format_value(v)
}

fn format_value(v: &Value) -> String {
    match v {
        Value::Real(x) => {
            if *x == x.floor() && x.abs() < 1_000_000.0 {
                format!("{}", *x as i64)
            } else {
                format!("{x:.4}")
            }
        }
        Value::Str(s)  => format!("\"{s}\""),
        Value::Sym(s)  => format!("'{s}"),
        Value::Nil     => "nil".into(),
        _              => String::new(),
    }
}
