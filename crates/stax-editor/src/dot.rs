/// D6 — Graphviz DOT export + viewer window.

use stax_graph::{Graph, PortKind};
use crate::shell;

/// Render a `Graph` as a Graphviz DOT string.
pub fn graph_to_dot(graph: &Graph) -> String {
    let mut out = String::from("digraph stax {\n");
    out.push_str("  graph [rankdir=TB fontname=\"monospace\" bgcolor=\"#f4f1ea\"];\n");
    out.push_str("  node  [shape=box fontname=\"monospace\" fontsize=11 style=filled fillcolor=\"#f4f1ea\" color=\"#1a1a1a\"];\n");
    out.push_str("  edge  [fontname=\"monospace\" fontsize=9];\n\n");

    for node in graph.nodes_in_order() {
        let label = node.label().replace('"', "\\\"");
        let is_sink = node.is_sink();
        let fill = if is_sink { "#ebe7dd" } else { "#f4f1ea" };
        out.push_str(&format!(
            "  n{} [label=\"{}\" fillcolor=\"{}\"",
            node.id.0, label, fill
        ));
        if is_sink { out.push_str(" penwidth=2"); }
        out.push_str("];\n");
    }

    out.push('\n');

    for edge in graph.edges() {
        let kind = graph.node(edge.src.node)
            .and_then(|n| n.outputs.get(edge.src.port as usize))
            .map(|p| p.kind)
            .unwrap_or(PortKind::Any);
        let (color, dashed) = dot_edge_style(&kind);
        out.push_str(&format!(
            "  n{} -> n{} [color=\"{}\"",
            edge.src.node.0, edge.dst.node.0, color
        ));
        if dashed { out.push_str(" style=dashed"); }
        out.push_str("];\n");
    }

    out.push_str("}\n");
    out
}

fn dot_edge_style(kind: &PortKind) -> (&'static str, bool) {
    match kind {
        PortKind::Real   => ("#1a1a1a", false),
        PortKind::Signal => ("#c94820", false),
        PortKind::Stream => ("#2d5a4a", true),
        PortKind::Fun    => ("#6b4e8a", false),
        PortKind::Form   => ("#8a6b2a", false),
        PortKind::Any    => ("#6b6558", false),
        PortKind::Str | PortKind::Sym => ("#6b6558", false),
    }
}

// ── Draw the DOT viewer egui::Window ──────────────────────────────────────

/// Draw the floating DOT viewer window.
/// `open` is set to false when the window is closed.
/// `dot_src` holds the current DOT source (refreshed by the Refresh button).
pub fn draw_dot_window(
    ctx: &egui::Context,
    open: &mut bool,
    dot_src: &mut String,
    graph: &Graph,
) {
    let mut still_open = *open;
    egui::Window::new("DOT source")
        .id(egui::Id::new("dot_window"))
        .default_size([520.0, 400.0])
        .resizable(true)
        .open(&mut still_open)
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(shell::PAPER)
                .stroke(egui::Stroke::new(1.0, shell::RULE)),
        )
        .show(ctx, |ui| {
            // Toolbar
            ui.horizontal(|ui| {
                if ui.small_button("Refresh").clicked() {
                    *dot_src = graph_to_dot(graph);
                }
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(dot_src.clone());
                }
                if ui.small_button("Save…").clicked() {
                    if let Ok(path) = std::env::current_dir().map(|d| d.join("graph.dot")) {
                        let _ = std::fs::write(&path, dot_src.as_bytes());
                    }
                }
                // Run dot if installed
                if ui.small_button("Run dot →  PNG").clicked() {
                    let tmp = std::env::temp_dir().join("stax_graph.dot");
                    let out = std::env::temp_dir().join("stax_graph.png");
                    if std::fs::write(&tmp, dot_src.as_bytes()).is_ok() {
                        let _ = std::process::Command::new("dot")
                            .args(["-Tpng", tmp.to_str().unwrap_or(""), "-o", out.to_str().unwrap_or("")])
                            .output();
                        // Open in default viewer
                        #[cfg(target_os = "windows")]
                        let _ = std::process::Command::new("cmd").args(["/c", "start", out.to_str().unwrap_or("")]).spawn();
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("open").arg(&out).spawn();
                        #[cfg(target_os = "linux")]
                        let _ = std::process::Command::new("xdg-open").arg(&out).spawn();
                    }
                }
            });

            ui.separator();

            // Scrollable source text (read-only display)
            egui::ScrollArea::both()
                .id_salt("dot_scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    let mut text = dot_src.clone();
                    let re = ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .font(egui::FontId::new(11.0, egui::FontFamily::Monospace))
                            .text_color(shell::INK)
                            .desired_width(f32::INFINITY)
                            .desired_rows(20),
                    );
                    if re.changed() {
                        *dot_src = text;
                    }
                });
        });

    *open = still_open;
}
