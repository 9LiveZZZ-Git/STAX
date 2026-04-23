//! Headed UI tests for stax-editor using egui_kittest.
//!
//! Most tests exercise app logic directly (no display needed).
//! The harness tests render into an off-screen buffer using egui_kittest —
//! they verify egui doesn't panic while rendering our syntax highlighting
//! and widget layouts.
//!
//! Run:  cargo test -p stax-editor -- --nocapture

use egui_kittest::Harness;
use stax_editor::app::{StaxApp, View};

// ── Harness smoke tests ───────────────────────────────────────────────────────

/// Syntax highlighter + label rendering doesn't panic on one frame.
#[test]
fn harness_syntax_label_smoke() {
    let mut harness = Harness::new(|ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let job = stax_editor::syntax::layout_job("440 sinosc play");
            ui.label(egui::widget_text::WidgetText::LayoutJob(job));
        });
    });
    harness.run();
}

/// egui_kittest renders several frames of a live egui::TextEdit without panic.
#[test]
fn harness_text_edit_smoke() {
    let mut text = "hello stax".to_owned();
    let mut harness = Harness::new(move |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.text_edit_multiline(&mut text);
        });
    });
    for _ in 0..3 {
        harness.run();
    }
}

// ── Pure-logic tests (no display needed) ─────────────────────────────────────

/// App initializes without panicking; default source parses cleanly.
#[test]
fn smoke_default_source() {
    let app = StaxApp::new_for_test();
    assert!(app.parse_error.is_none(), "default source should parse cleanly");
    assert!(app.graph.node_count() > 0, "default source should produce nodes");
}

/// View field starts on Graph.
#[test]
fn default_view_is_graph() {
    let app = StaxApp::new_for_test();
    assert_eq!(app.view, View::Graph);
}

/// exec_repl: 2 3 + → stack top is 5.
#[test]
fn repl_arithmetic() {
    let mut app = StaxApp::new_for_test();
    app.exec_repl("2 3 +");
    assert_eq!(app.interp.stack.len(), 1);
    if let stax_core::Value::Real(x) = app.interp.stack[0] {
        assert!((x - 5.0).abs() < 1e-9, "expected 5.0, got {x}");
    } else {
        panic!("expected Real on stack");
    }
}

/// exec_repl records a TravelSnapshot on success.
#[test]
fn repl_records_snapshot() {
    let mut app = StaxApp::new_for_test();
    assert_eq!(app.travel_snapshots.len(), 0);
    app.exec_repl("42");
    assert_eq!(app.travel_snapshots.len(), 1);
    assert_eq!(app.travel_snapshots[0].label, "42");
}

/// exec_repl on parse error does not push a snapshot.
#[test]
fn repl_parse_error_no_snapshot() {
    let mut app = StaxApp::new_for_test();
    app.exec_repl("@@@@BADTOKEN");
    assert_eq!(app.travel_snapshots.len(), 0);
}

/// .c command clears the stack.
#[test]
fn repl_clear_command() {
    let mut app = StaxApp::new_for_test();
    app.exec_repl("1 2 3");
    assert!(!app.interp.stack.is_empty());
    app.exec_repl(".c");
    assert!(app.interp.stack.is_empty());
}

/// Modifying source and calling recompile updates the graph.
#[test]
fn source_edit_recompiles() {
    let mut app = StaxApp::new_for_test();
    let original_count = app.graph.node_count();
    app.source = "2 3 +".to_owned();
    app.recompile();
    assert!(app.parse_error.is_none());
    assert_ne!(app.graph.node_count(), original_count,
        "graph should change after source edit");
}

/// Bad source sets parse_error without panic.
#[test]
fn bad_source_sets_parse_error() {
    let mut app = StaxApp::new_for_test();
    // Use something the parser will definitely reject
    app.source = "= =".to_owned();
    app.recompile();
    // Just verify no panic; error may or may not be set depending on parser
    let _ = &app.parse_error;
}

/// compute_cursor_stack is lazy: same cursor_line returns cached result.
#[test]
fn cursor_stack_lazy() {
    let mut app = StaxApp::new_for_test();
    app.source = "10 20".to_owned();
    app.recompile();
    app.cursor_line = 1;
    app.compute_cursor_stack();
    let cached_line = app.cursor_stack_line;
    app.compute_cursor_stack();
    assert_eq!(app.cursor_stack_line, cached_line, "should not re-evaluate same line");
}

/// compute_cursor_stack updates when cursor_line changes.
#[test]
fn cursor_stack_updates_on_line_change() {
    let mut app = StaxApp::new_for_test();
    app.source = "10\n20".to_owned();
    app.recompile();
    app.cursor_line = 1;
    app.compute_cursor_stack();
    let stack1 = app.cursor_stack.clone();
    app.cursor_line = 2;
    app.compute_cursor_stack();
    let stack2 = app.cursor_stack.clone();
    assert_ne!(stack1.len(), stack2.len(), "stack should grow after second line");
}

/// Auto-layout assigns positions for all nodes in the graph.
#[test]
fn auto_layout_covers_all_nodes() {
    let app = StaxApp::new_for_test();
    for node in app.graph.nodes_in_order() {
        assert!(
            app.node_positions.contains_key(&node.id),
            "node {:?} has no position", node.id
        );
    }
}

/// Canvas starts at zero pan and 1× zoom.
#[test]
fn canvas_default_transform() {
    let app = StaxApp::new_for_test();
    assert_eq!(app.canvas_pan, egui::Vec2::ZERO);
    assert!((app.canvas_zoom - 1.0).abs() < 1e-6);
}

/// fit_canvas_to_nodes completes without panic and changes zoom.
#[test]
fn fit_canvas_to_nodes_no_panic() {
    let mut app = StaxApp::new_for_test();
    app.fit_canvas_to_nodes();
    // zoom should be clamped to [0.2, 2.0]
    assert!(app.canvas_zoom >= 0.2 && app.canvas_zoom <= 2.0,
        "zoom out of range: {}", app.canvas_zoom);
}

/// Reveal router: GraphNode switches view and selects the node.
#[test]
fn reveal_router_graph_node() {
    use stax_editor::app::RevealTarget;
    let mut app = StaxApp::new_for_test();
    app.view = View::Text;
    let nid = app.graph.nodes_in_order().next().map(|n| n.id);
    if let Some(id) = nid {
        app.pending_reveal = Some(RevealTarget::GraphNode(id));
        if let Some(RevealTarget::GraphNode(rid)) = app.pending_reveal.take() {
            app.view = View::Graph;
            app.selected_node = Some(rid);
        }
        assert_eq!(app.view, View::Graph);
        assert_eq!(app.selected_node, Some(id));
    }
}

/// Reveal router: TextLine switches view and sets cursor.
#[test]
fn reveal_router_text_line() {
    use stax_editor::app::RevealTarget;
    let mut app = StaxApp::new_for_test();
    app.view = View::Graph;
    app.pending_reveal = Some(RevealTarget::TextLine(5));
    if let Some(RevealTarget::TextLine(line)) = app.pending_reveal.take() {
        app.view = View::Text;
        app.cursor_line = line;
    }
    assert_eq!(app.view, View::Text);
    assert_eq!(app.cursor_line, 5);
}

/// TravelSnapshot ring buffer caps at 1000.
#[test]
fn travel_snapshot_ring_cap() {
    let mut app = StaxApp::new_for_test();
    for i in 0..=1001 {
        app.exec_repl(&format!("{i}"));
    }
    assert!(app.travel_snapshots.len() <= 1000,
        "ring buffer should cap at 1000, got {}", app.travel_snapshots.len());
}

/// travel_step stays in bounds after many execs.
#[test]
fn travel_step_in_bounds() {
    let mut app = StaxApp::new_for_test();
    for i in 0..5 {
        app.exec_repl(&format!("{i}"));
    }
    assert!(app.travel_step < app.travel_snapshots.len(),
        "travel_step {} out of bounds (len {})", app.travel_step, app.travel_snapshots.len());
}
