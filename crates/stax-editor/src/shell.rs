use egui::Color32;

// ── Design token colors ────────────────────────────────────────────────────
pub const PAPER: Color32 = Color32::from_rgb(0xf4, 0xf1, 0xea);
pub const PAPER_2: Color32 = Color32::from_rgb(0xef, 0xeb, 0xe1);
pub const SURFACE: Color32 = Color32::from_rgb(0xeb, 0xe7, 0xdd);
pub const RULE: Color32 = Color32::from_rgb(0xd4, 0xcf, 0xc3);
pub const RULE_2: Color32 = Color32::from_rgb(0xbc, 0xb5, 0xa4);
pub const INK: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
pub const INK_2: Color32 = Color32::from_rgb(0x6b, 0x65, 0x58);
pub const INK_3: Color32 = Color32::from_rgb(0x9a, 0x93, 0x83);
pub const WARM: Color32 = Color32::from_rgb(0xc9, 0x48, 0x20);
pub const COOL: Color32 = Color32::from_rgb(0x2d, 0x5a, 0x4a);
/// Error/diagnostic red — distinct from WARM; matches design --err: #b03a2e
pub const ERR: Color32 = Color32::from_rgb(0xb0, 0x3a, 0x2e);

// Port colors — identical to design tokens; appear nowhere else in the UI
pub const PORT_REAL: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
pub const PORT_SIGNAL: Color32 = Color32::from_rgb(0xc9, 0x48, 0x20);
pub const PORT_STREAM: Color32 = Color32::from_rgb(0x2d, 0x5a, 0x4a);
pub const PORT_FUN: Color32 = Color32::from_rgb(0x6b, 0x4e, 0x8a);
pub const PORT_FORM: Color32 = Color32::from_rgb(0x8a, 0x6b, 0x2a);

// ── Layout dimensions (px) ─────────────────────────────────────────────────
pub const HEADER_H: f32 = 40.0;
pub const TABS_H: f32 = 28.0;
pub const LIB_W: f32 = 200.0;
pub const INSP_W: f32 = 240.0;
pub const SIDE_W: f32 = 280.0; // text view right panel
pub const REPL_H: f32 = 120.0;
pub const TIMEBAR_H: f32 = 34.0;
pub const BOTBAR_H: f32 = 28.0;

// ── Node geometry ──────────────────────────────────────────────────────────
pub const NODE_MIN_W: f32 = 88.0;
pub const NODE_HDR_H: f32 = 28.0;
pub const NODE_SUB_H: f32 = 18.0;
pub const PORT_HALF: f32 = 5.0; // half side of the port square

/// Wire/port visual spec for a given PortKind.
/// Returns (color, is_dashed, stroke_width).
pub fn port_style(kind: &stax_graph::PortKind) -> (Color32, bool, f32) {
    use stax_graph::PortKind::*;
    match kind {
        Real => (PORT_REAL, false, 1.4),
        Signal => (PORT_SIGNAL, false, 1.4),
        Stream => (PORT_STREAM, true, 1.4),
        Fun => (PORT_FUN, false, 2.0),
        Form => (PORT_FORM, false, 1.4),
        Str => (PORT_STREAM, false, 1.4),
        Sym => (PORT_FUN, false, 1.4),
        Any => (INK_2, false, 1.0),
    }
}
