use egui::{pos2, vec2, Rect, Stroke};
use stax_core::Value;
use crate::{app::StaxApp, shell};

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

    // ── Centre: code editor with syntax highlighting ───────────────────────

    pub(crate) fn draw_text_editor(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);

        // Breadcrumb bar
        let bc_h = 24.0;
        let bc_rect = Rect::from_min_size(rect.min, vec2(rect.width(), bc_h));
        ui.painter().rect_filled(bc_rect, 0.0, shell::PAPER_2);
        ui.painter().line_segment(
            [bc_rect.left_bottom(), bc_rect.right_bottom()],
            Stroke::new(0.5, shell::RULE_2),
        );
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

        // Code area — editable TextEdit with syntax highlight overlay
        let code_rect = Rect::from_min_size(
            pos2(rect.min.x, rect.min.y + bc_h),
            vec2(rect.width(), rect.height() - bc_h),
        );

        let mut ui_child = ui.new_child(egui::UiBuilder::new().max_rect(code_rect).layout(egui::Layout::top_down(egui::Align::LEFT)));

        egui::ScrollArea::vertical().show(&mut ui_child, |ui| {
            ui.set_width(code_rect.width());

            // Line numbers + highlighted source
            let lines: Vec<&str> = self.source.lines().collect();
            let ln_w   = 36.0;
            let gutter = 14.0;

            for (i, line) in lines.iter().enumerate() {
                let ln = i + 1;
                let active = ln == self.cursor_line;

                // Paint row background BEFORE content so text renders on top
                if active {
                    let row_rect = Rect::from_min_size(
                        ui.cursor().min,
                        vec2(ui.available_width(), 18.0),
                    );
                    ui.painter().rect_filled(row_rect, 0.0, shell::PAPER_2);
                }

                ui.horizontal(|ui| {
                    // Live indicator dot (empty for now — future: mark executing lines)
                    ui.add_space(16.0);

                    // Line number
                    ui.add_sized(
                        vec2(ln_w, 18.0),
                        egui::Label::new(
                            egui::RichText::new(format!("{ln}"))
                                .color(if active { shell::INK } else { shell::INK_3 })
                                .size(11.0)
                                .monospace(),
                        ),
                    );

                    // Gutter space
                    ui.add_space(gutter);

                    // Syntax-highlighted line
                    let job = crate::syntax::layout_job(line);
                    ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Extend));
                });
            }
        });

        // Plain text edit (invisible, behind the highlighted display) for cursor tracking
        // For M5 the source is edited via the REPL.  Full cursor editing is M6.
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

            // ── STACK ──
            section_header(ui, "stack", None);
            draw_stack_contents(ui, &self.interp.stack);

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

    // ── outline helper ─────────────────────────────────────────────────────

    fn outline_bindings(&self) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        for (line_idx, line) in self.source.lines().enumerate() {
            let trimmed = line.trim();
            // Detect "expr = name" pattern (bind at end of line)
            if let Some(pos) = trimmed.rfind(" = ") {
                let name = &trimmed[pos + 3..];
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                    out.push((name.to_owned(), line_idx + 1));
                }
            }
        }
        out
    }
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
        // Dotted underline
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
                0.0, shell::SURFACE,
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
                egui::RichText::new("(empty)").color(shell::INK_3).size(11.0).monospace(),
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

fn value_kind_color(v: &Value) -> (egui::Color32, bool) {
    match v {
        Value::Real(_)   => (shell::PORT_REAL,   false),
        Value::Signal(_) => (shell::PORT_SIGNAL, false),
        Value::Stream(_) => (shell::PORT_STREAM, true),
        Value::Fun(_)    => (shell::PORT_FUN,    false),
        Value::Form(_)   => (shell::PORT_FORM,   false),
        _                => (shell::INK_2,       false),
    }
}

pub fn format_value_pub(v: &Value) -> String { format_value(v) }

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
