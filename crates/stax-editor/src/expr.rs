use egui::text::LayoutJob;
use stax_core::Op;

/// Render a slice of `Op`s as a syntax-highlighted `LayoutJob`.
///
/// Uses the same token colors as `crate::syntax::layout_job` so the
/// expression looks identical whether it appears in the text editor or in an
/// inspector / fn-port panel.
pub fn ops_layout_job(ops: &[Op]) -> LayoutJob {
    let src = ops_to_source(ops);
    crate::syntax::layout_job(&src)
}

/// Convert a `Vec<Op>` back to a canonical postfix source string.
///
/// This is a lightweight pretty-printer, not a full round-trip lowerer (that
/// lives in `stax_graph::lower`).  It produces human-readable text for
/// inspector panels, tooltips, and the fn-port text-equivalent row.
pub fn ops_to_source(ops: &[Op]) -> String {
    let mut out = String::new();
    for op in ops {
        if !out.is_empty() {
            out.push(' ');
        }
        match op {
            Op::Lit(v) => {
                use stax_core::Value;
                match v {
                    Value::Real(x) => {
                        if *x == x.floor() && x.abs() < 1_000_000.0 {
                            out.push_str(&format!("{}", *x as i64));
                        } else {
                            out.push_str(&format!("{x}"));
                        }
                    }
                    Value::Str(s)  => { out.push('"'); out.push_str(s); out.push('"'); }
                    Value::Sym(s)  => { out.push('\''); out.push_str(s); }
                    Value::Nil     => out.push_str("nil"),
                    _              => out.push_str("<val>"),
                }
            }
            Op::Word(w)          => out.push_str(w),
            Op::Bind(name)       => { out.push_str("= "); out.push_str(name); }
            Op::Quote(name)      => { out.push('`'); out.push_str(name); }
            Op::Sym(name)        => { out.push('\''); out.push_str(name); }
            Op::Call             => out.push('!'),
            Op::MakeList { .. }  => out.push_str("[…]"),
            Op::MakeForm { .. }  => out.push_str("{…}"),
            Op::MakeFun { params, .. } => {
                out.push('\\');
                let p: Vec<&str> = params.iter().map(|s| s.as_ref()).collect();
                out.push_str(&p.join(" "));
                out.push_str(" […]");
            }
            Op::Adverb(adv) => {
                use stax_core::op::Adverb;
                let s = match adv {
                    Adverb::Reduce   => "/",
                    Adverb::Scan     => "\\",
                    Adverb::Pairwise => "^",
                };
                // Adverb attaches to the next word; strip the trailing space
                if out.ends_with(' ') { out.pop(); }
                out.push_str(s);
            }
            _ => out.push_str("<op>"),
        }
    }
    out
}
