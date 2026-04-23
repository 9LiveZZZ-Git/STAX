use crate::{
    app::{ReplKind, StaxApp},
    shell,
};
use egui::{vec2, Color32, Stroke};
use stax_core::{op::Adverb, Op, Value};
use stax_graph::NodeKind;

// ── Op formatter ─────────────────────────────────────────────────────────────

fn op_row(op: &Op) -> (&'static str, String) {
    match op {
        Op::Lit(v) => ("push", fmt_val(v)),
        Op::Word(w) => ("word", w.to_string()),
        Op::Quote(w) => ("quote", format!("`{w}")),
        Op::Sym(w) => ("sym", format!("'{w}")),
        Op::FormGet(w) => ("get", format!(",{w}")),
        Op::FormApply(w) => ("apply", format!(".{w}")),
        Op::Bind(w) => ("bind", format!("= {w}")),
        Op::BindMany { names, list_mode } => {
            let ns = names
                .iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>()
                .join(" ");
            let mode = if *list_mode { "[" } else { "(" };
            ("bindmany", format!("{mode}{ns}"))
        }
        Op::Call => ("call", "!".into()),
        Op::ListMark => ("mark", "[".into()),
        Op::MakeList { signal } => ("list", if *signal { "~sig" } else { "~val" }.into()),
        Op::MakeForm { keys, parent } => {
            let ks = keys
                .iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>()
                .join(" ");
            (
                "form",
                if *parent {
                    format!("^{{{ks}}}")
                } else {
                    format!("{{{ks}}}")
                },
            )
        }
        Op::MakeFun { params, body } => {
            let ps = params
                .iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>()
                .join(" ");
            ("fun", format!("\\{ps}  [{} ops]", body.len()))
        }
        Op::Each { depth, order } => ("each", format!("@depth={depth} ord={order}")),
        Op::Adverb(a) => (
            "adverb",
            match a {
                Adverb::Reduce => "/",
                Adverb::Scan => "\\",
                Adverb::Pairwise => "^",
            }
            .into(),
        ),
    }
}

fn node_kind_tag(k: &NodeKind) -> &'static str {
    match k {
        NodeKind::Literal(_) => "lit",
        NodeKind::Word(_) => "word",
        NodeKind::Quote(_) => "quote",
        NodeKind::Sym(_) => "sym",
        NodeKind::FormGet(_) => "form.get",
        NodeKind::FormApply(_) => "form.app",
        NodeKind::Bind(_) => "bind",
        NodeKind::BindMany { .. } => "bindmany",
        NodeKind::Call => "call",
        NodeKind::ListMark => "mark",
        NodeKind::MakeList { .. } => "list",
        NodeKind::MakeForm { .. } => "form",
        NodeKind::MakeFun { .. } => "fun",
    }
}

fn fmt_val(v: &Value) -> String {
    match v {
        Value::Real(x) => {
            if *x == x.floor() && x.abs() < 1_000_000.0 {
                format!("{}", *x as i64)
            } else {
                format!("{x:.4}")
            }
        }
        Value::Str(s) => format!("\"{s}\""),
        Value::Sym(s) => format!("'{s}"),
        Value::Nil => "nil".into(),
        Value::Stream(_) => "<stream>".into(),
        Value::Signal(_) => "<signal>".into(),
        Value::Form(_) => "<form>".into(),
        Value::Fun(_) => "<fun>".into(),
        Value::Ref(_) => "<ref>".into(),
    }
}

fn val_kind(v: &Value) -> &'static str {
    match v {
        Value::Real(_) => "real",
        Value::Str(_) => "str",
        Value::Sym(_) => "sym",
        Value::Stream(_) => "stream",
        Value::Signal(_) => "signal",
        Value::Form(_) => "form",
        Value::Fun(_) => "fun",
        Value::Ref(_) => "ref",
        Value::Nil => "nil",
    }
}

// ── Main debug view ───────────────────────────────────────────────────────────

impl StaxApp {
    pub(crate) fn draw_debug_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, shell::PAPER);

        egui::ScrollArea::vertical()
            .id_salt("dbg_scroll")
            .show(ui, |ui| {
                ui.set_min_width(rect.width());
                ui.add_space(8.0);

                // ── APP STATE ─────────────────────────────────────────────
                section(ui, "APP STATE");
                kv_table(
                    ui,
                    "dbg_app",
                    &[
                        ("view", format!("{:?}", self.view)),
                        (
                            "canvas_pan",
                            format!("{:.1},  {:.1}", self.canvas_pan.x, self.canvas_pan.y),
                        ),
                        ("canvas_zoom", format!("{:.3}×", self.canvas_zoom)),
                        ("anim_t", format!("{:.2} s", self.anim_t)),
                        ("source_mod", format!("{}", self.source_modified)),
                        ("cursor_line", format!("{}", self.cursor_line)),
                        ("selected_node", format!("{:?}", self.selected_node)),
                        (
                            "positions",
                            format!("{} nodes placed", self.node_positions.len()),
                        ),
                        ("rank_ovr", format!("{} entries", self.rank_overrides.len())),
                        (
                            "adverb_ovr",
                            format!("{} entries", self.adverb_overrides.len()),
                        ),
                    ],
                );

                // ── SOURCE ───────────────────────────────────────────────
                let (status_txt, status_col) = match &self.parse_error {
                    None => ("✓  clean".into(), shell::COOL),
                    Some(e) => (format!("✕  {e}"), shell::ERR),
                };
                section(
                    ui,
                    &format!(
                        "SOURCE   ({} chars, {} lines)",
                        self.source.len(),
                        self.source.lines().count()
                    ),
                );
                row(ui, "status", &status_txt, status_col);
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(shell::PAPER_2)
                    .stroke(Stroke::new(1.0, shell::RULE))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(rect.width() - 28.0);
                        egui::ScrollArea::vertical()
                            .id_salt("dbg_src")
                            .max_height(96.0)
                            .show(ui, |ui| {
                                let job = crate::syntax::layout_job(&self.source);
                                ui.label(egui::widget_text::WidgetText::LayoutJob(job));
                            });
                    });

                // ── OPS (IR) ─────────────────────────────────────────────
                let ops = self.ops.clone();
                section(ui, &format!("IR   ({} ops)", ops.len()));
                if ops.is_empty() {
                    empty(ui, "(no ops — source must be empty or errored)");
                } else {
                    egui::Frame::new()
                        .fill(shell::PAPER_2)
                        .stroke(Stroke::new(1.0, shell::RULE))
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("dbg_ops")
                                .max_height(160.0)
                                .show(ui, |ui| {
                                    egui::Grid::new("dbg_ops_grid")
                                        .striped(true)
                                        .num_columns(3)
                                        .min_col_width(28.0)
                                        .show(ui, |ui| {
                                            for (i, op) in ops.iter().enumerate() {
                                                let (kind, detail) = op_row(op);
                                                mono(ui, &format!("{i:>3}"), shell::INK_3, 10.0);
                                                mono(ui, kind, shell::PORT_FUN, 11.0);
                                                mono(ui, &detail, shell::INK, 11.0);
                                                ui.end_row();
                                            }
                                        });
                                });
                        });
                }

                // ── GRAPH ────────────────────────────────────────────────
                let nodes: Vec<_> = self
                    .graph
                    .nodes_in_order()
                    .map(|n| {
                        (
                            n.id.0,
                            node_kind_tag(&n.kind),
                            n.label(),
                            n.inputs.len(),
                            n.outputs.len(),
                            n.adverb,
                        )
                    })
                    .collect();
                let edge_count = self.graph.edge_count();
                section(
                    ui,
                    &format!("GRAPH   ({} nodes,  {} edges)", nodes.len(), edge_count),
                );
                if nodes.is_empty() {
                    empty(ui, "(no nodes)");
                } else {
                    egui::Frame::new()
                        .fill(shell::PAPER_2)
                        .stroke(Stroke::new(1.0, shell::RULE))
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("dbg_graph")
                                .max_height(200.0)
                                .show(ui, |ui| {
                                    egui::Grid::new("dbg_graph_grid")
                                        .striped(true)
                                        .num_columns(6)
                                        .min_col_width(24.0)
                                        .show(ui, |ui| {
                                            // Header
                                            for h in &["id", "kind", "label", "in", "out", "adv"] {
                                                mono(ui, h, shell::INK_3, 9.0);
                                            }
                                            ui.end_row();

                                            for (id, kind, label, ins, outs, adv) in &nodes {
                                                mono(ui, &format!("{id}"), shell::INK_3, 10.0);
                                                mono(ui, kind, shell::INK_2, 10.0);
                                                mono_strong(ui, label, shell::INK, 11.0);
                                                mono(
                                                    ui,
                                                    &format!("{ins}"),
                                                    shell::PORT_STREAM,
                                                    10.0,
                                                );
                                                mono(
                                                    ui,
                                                    &format!("{outs}"),
                                                    shell::PORT_SIGNAL,
                                                    10.0,
                                                );
                                                let adv_str = match adv {
                                                    None => "-",
                                                    Some(Adverb::Reduce) => "/",
                                                    Some(Adverb::Scan) => "\\",
                                                    Some(Adverb::Pairwise) => "^",
                                                };
                                                mono(ui, adv_str, shell::PORT_FUN, 10.0);
                                                ui.end_row();
                                            }
                                        });
                                });
                        });
                }

                // ── STACK ────────────────────────────────────────────────
                let stack: Vec<_> = self
                    .interp
                    .stack
                    .iter()
                    .rev()
                    .map(|v| (val_kind(v), fmt_val(v)))
                    .collect();
                section(ui, &format!("STACK   ({} values)", stack.len()));
                if stack.is_empty() {
                    empty(ui, "(empty)");
                } else {
                    egui::Grid::new("dbg_stack_grid")
                        .striped(true)
                        .num_columns(3)
                        .min_col_width(28.0)
                        .show(ui, |ui| {
                            for (i, (kind, val)) in stack.iter().enumerate() {
                                mono(ui, &format!("{i}"), shell::INK_3, 10.0);
                                mono(ui, kind, shell::PORT_SIGNAL, 10.0);
                                mono(ui, val, shell::INK, 11.0);
                                ui.end_row();
                            }
                        });
                }

                // ── TIME-TRAVEL ──────────────────────────────────────────
                let total = self.travel_snapshots.len();
                let step = self.travel_step;
                section(
                    ui,
                    &format!(
                        "TIME-TRAVEL   (step {}/{total})",
                        if total == 0 { 0 } else { step + 1 }
                    ),
                );
                if total == 0 {
                    empty(ui, "(no snapshots — run REPL commands to capture)");
                } else {
                    let start = total.saturating_sub(30);
                    egui::Grid::new("dbg_tt_grid")
                        .striped(true)
                        .num_columns(3)
                        .min_col_width(28.0)
                        .show(ui, |ui| {
                            for (i, snap) in self.travel_snapshots[start..].iter().enumerate() {
                                let abs = start + i;
                                let is_cur = abs == step;
                                let c = if is_cur { shell::WARM } else { shell::INK_3 };
                                mono(ui, if is_cur { "▶" } else { " " }, c, 10.0);
                                mono(ui, &format!("{abs}"), c, 10.0);
                                mono(
                                    ui,
                                    &snap.label,
                                    if is_cur { shell::WARM } else { shell::INK_2 },
                                    11.0,
                                );
                                ui.end_row();
                            }
                        });
                }

                // ── REPL HISTORY ─────────────────────────────────────────
                let history = self.repl_history.clone();
                section(ui, &format!("REPL HISTORY   ({} entries)", history.len()));
                egui::Frame::new()
                    .fill(shell::PAPER_2)
                    .stroke(Stroke::new(1.0, shell::RULE))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("dbg_repl")
                            .max_height(140.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                ui.set_min_width(rect.width() - 28.0);
                                for entry in &history {
                                    let (pfx, col) = match entry.kind {
                                        ReplKind::Input => ("›", shell::INK),
                                        ReplKind::Output => (" ", shell::INK_2),
                                        ReplKind::Ok => ("✓", shell::COOL),
                                        ReplKind::Err => ("✕", shell::ERR),
                                    };
                                    ui.label(
                                        egui::RichText::new(format!("{pfx}  {}", entry.text))
                                            .color(col)
                                            .size(11.0)
                                            .monospace(),
                                    );
                                }
                            });
                    });

                ui.add_space(20.0);
            });
    }
}

// ── Widget helpers ────────────────────────────────────────────────────────────

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 20.0));
    ui.painter().rect_filled(rect, 0.0, shell::SURFACE);
    ui.painter().line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, shell::RULE),
    );
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, shell::RULE),
    );
    ui.painter().text(
        egui::pos2(rect.min.x + 14.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::new(10.0, egui::FontFamily::Monospace),
        shell::INK_3,
    );
    ui.add_space(4.0);
}

fn kv_table(ui: &mut egui::Ui, id: &str, rows: &[(&str, String)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(110.0)
        .show(ui, |ui| {
            for (k, v) in rows {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    mono(ui, k, shell::INK_2, 11.0);
                });
                mono(ui, v, shell::INK, 11.0);
                ui.end_row();
            }
        });
}

fn row(ui: &mut egui::Ui, key: &str, val: &str, col: Color32) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add_sized(
            vec2(80.0, 16.0),
            egui::Label::new(
                egui::RichText::new(key)
                    .color(shell::INK_2)
                    .size(11.0)
                    .monospace(),
            ),
        );
        ui.label(egui::RichText::new(val).color(col).size(11.0).monospace());
    });
}

fn mono(ui: &mut egui::Ui, text: &str, col: Color32, size: f32) {
    ui.label(egui::RichText::new(text).color(col).size(size).monospace());
}

fn mono_strong(ui: &mut egui::Ui, text: &str, col: Color32, size: f32) {
    ui.label(
        egui::RichText::new(text)
            .color(col)
            .size(size)
            .monospace()
            .strong(),
    );
}

fn empty(ui: &mut egui::Ui, msg: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(msg)
                .color(shell::INK_3)
                .size(11.0)
                .monospace(),
        );
    });
}
