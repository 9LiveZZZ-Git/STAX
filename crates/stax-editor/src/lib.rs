// stax-editor — M5
//
// egui app shell + six views over a single .stax file.
// Custom canvas on egui primitives (not egui_node_graph).
//
// Shared services (built once, used by all views):
//   - shell:  top bar, tabs, 240px inspector, 28px status bar, layout tokens
//   - syntax: lex &str -> semantic spans (color scheme = design tokens)
//   - expr:   render &[Op] to highlighted text (canonical code display)
//   - repl:   embeddable REPL pane sharing a parent Interp
//   - scope:  drop-in scope/spectrum/meter widget for any Signal
//   - reveal: cross-view jump service (SourceRange -> ViewId -> Selection)
//
// Views (M5): text (06), graph (01), fn-port (02)
// Views (M7): arrangement (03), session (04), mixer (05)

pub mod shell {}
pub mod syntax {}
pub mod expr {}
pub mod repl {}
pub mod scope {}
pub mod reveal {}
