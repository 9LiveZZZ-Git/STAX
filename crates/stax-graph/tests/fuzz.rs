//! Graph IR fuzz tests — lift/lower round-trip and semantic equivalence.
//!
//! Each test:
//!   1. Parses a stax program into `Vec<Op>` (the canonical IR).
//!   2. Lifts it to a `Graph`.
//!   3. Lowers back to `Vec<Op>`.
//!   4. Evaluates both the original and the lowered ops with `Interp`.
//!   5. Asserts semantic equivalence (same stack depth, same values).

use stax_core::{op::Adverb, Op, Value};
use stax_eval::Interp;
use stax_graph::{lift, lower, topo_sort, NodeKind};
use stax_parser::parse;

// ── helpers ──────────────────────────────────────────────────────────────────

fn run(ops: &[Op]) -> Vec<Value> {
    let mut i = Interp::new();
    i.exec(ops).expect("exec failed");
    i.stack
}

fn real(v: &Value) -> f64 {
    v.as_real().expect("expected Real")
}

fn assert_real_eq(a: f64, b: f64, label: &str) {
    assert!(
        (a - b).abs() < 1e-9,
        "{label}: expected {a} == {b}, diff = {}",
        (a - b).abs()
    );
}

/// Parse, lift, lower, run both. Return (original_stack, lowered_stack).
fn roundtrip(src: &str) -> (Vec<Value>, Vec<Value>) {
    let orig = parse(src).expect("parse failed");
    let graph = lift(&orig);
    let lowered = lower(&graph);
    let r1 = run(&orig);
    let r2 = run(&lowered);
    (r1, r2)
}

/// Parse, lift, lower, run both. Assert stacks have same number of reals,
/// each real within 1e-9 of the other.
fn assert_roundtrip_reals(src: &str) {
    let (r1, r2) = roundtrip(src);
    assert_eq!(
        r1.len(),
        r2.len(),
        "stack depth mismatch for `{src}`: orig={} lowered={}",
        r1.len(),
        r2.len()
    );
    for (i, (v1, v2)) in r1.iter().zip(r2.iter()).enumerate() {
        assert_real_eq(real(v1), real(v2), &format!("`{src}` stack[{i}]"));
    }
}

// ── Test 1: Form construction {`:x 10 :y 20`} ────────────────────────────────

#[test]
fn roundtrip_form_construction() {
    let src = "{:x 10 :y 20}";
    let orig = parse(src).expect("parse failed");
    let graph = lift(&orig);
    let lowered = lower(&graph);

    let r1 = run(&orig);
    let r2 = run(&lowered);

    assert_eq!(
        r1.len(),
        1,
        "form construction should leave 1 value on stack"
    );
    assert_eq!(r2.len(), 1);

    // Both should be Forms
    match (&r1[0], &r2[0]) {
        (Value::Form(_), Value::Form(_)) => {}
        _ => panic!("expected Form on stack for `{src}`"),
    }
}

// ── Test 2: Multi-bind `1 2 3 = (a b c)` ─────────────────────────────────────

#[test]
fn roundtrip_multi_bind() {
    // Bind 3 values and read them back
    let src = "1 2 3 = (a b c)  a b c";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    // Expect [1, 2, 3] on stack
    assert_eq!(r1.len(), 3);
    assert_real_eq(real(&r1[0]), 1.0, "a");
    assert_real_eq(real(&r1[1]), 2.0, "b");
    assert_real_eq(real(&r1[2]), 3.0, "c");
}

// ── Test 3: Lambda bind and call ─────────────────────────────────────────────

#[test]
fn roundtrip_lambda_bind_call() {
    // \a [a a +] = double  5 `double ! → 10
    let src = "\\a [a a +] = double  5 `double !";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    assert_real_eq(real(&r1[0]), 10.0, "double(5)");
}

// ── Test 4: Nested list `[[1 2] [3 4] [5 6]]` ────────────────────────────────

#[test]
fn roundtrip_nested_list() {
    let src = "[[1 2] [3 4] [5 6]]";
    let orig = parse(src).expect("parse failed");
    let graph = lift(&orig);
    let lowered = lower(&graph);

    let r1 = run(&orig);
    let r2 = run(&lowered);

    assert_eq!(r1.len(), 1, "nested list should leave 1 stream on stack");
    assert_eq!(r2.len(), 1);

    // Both should be streams (lists are Streams in stax)
    match (&r1[0], &r2[0]) {
        (Value::Stream(_), Value::Stream(_)) => {}
        _ => panic!(
            "expected Stream for nested list, got {:?} / {:?}",
            r1[0].kind(),
            r2[0].kind()
        ),
    }

    // Both should produce the same op count
    assert_eq!(orig.len(), lowered.len(), "op count should be preserved");
}

// ── Test 5: Stream take `[1 2 3 4 5] 3 N` ───────────────────────────────────

#[test]
fn roundtrip_stream_take() {
    let src = "[1 2 3 4 5] 3 N";
    let orig = parse(src).expect("parse failed");
    let graph = lift(&orig);
    let lowered = lower(&graph);

    assert_eq!(orig.len(), lowered.len(), "op count preserved");

    let r1 = run(&orig);
    let r2 = run(&lowered);
    assert_eq!(r1.len(), r2.len(), "stack depth preserved");
}

// ── Test 6: Stream construction `1 10 to` ────────────────────────────────────

#[test]
fn roundtrip_stream_to() {
    let src = "1 10 to";
    let orig = parse(src).expect("parse failed");
    let graph = lift(&orig);
    let lowered = lower(&graph);

    assert_eq!(orig.len(), lowered.len(), "op count preserved");

    let r1 = run(&orig);
    let r2 = run(&lowered);
    assert_eq!(r1.len(), r2.len(), "1..10 stream should leave 1 value");

    // Both should be streams
    assert!(matches!(r1[0], Value::Stream(_)));
    assert!(matches!(r2[0], Value::Stream(_)));
}

// ── Test 7: Multi-arg lambda `\x y [x y + x y - *] = f  3 4 `f !` ───────────

#[test]
fn roundtrip_multi_arg_lambda() {
    // (3+4) * (3-4) = 7 * -1 = -7
    let src = "\\x y [x y + x y - *] = f  3 4 `f !";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    assert_real_eq(real(&r1[0]), -7.0, "f(3,4)");
}

// ── Test 8: Stack ops `5 dup dup * *` (5^3 = 125) ────────────────────────────

#[test]
fn roundtrip_dup_cubed() {
    let src = "5 dup dup * *";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    assert_real_eq(real(&r1[0]), 125.0, "5 dup dup * *");
}

// ── Test 9: Symbol pushed through `'hello` ───────────────────────────────────

#[test]
fn roundtrip_sym_push() {
    let src = "'hello";
    let orig = parse(src).expect("parse failed");
    let lowered = lower(&lift(&orig));
    let r1 = run(&orig);
    let r2 = run(&lowered);

    assert_eq!(r1.len(), r2.len());
    match (&r1[0], &r2[0]) {
        (Value::Sym(a), Value::Sym(b)) => assert_eq!(a, b, "sym name should be preserved"),
        _ => panic!("expected Sym on stack"),
    }
}

// ── Test 10: Quote pushed through `` `double `` — op-level identity and call ──
//
// `` `name `` looks up `name` in the current Interp environment. Built-in words
// like `neg` are stored as OpWords, not named bindings, so `` `neg `` gives
// Unbound("neg"). The correct pattern is to first bind a lambda (`= double`) and
// then quote-call it (`` `double ! ``). This test exercises that pattern.

#[test]
fn roundtrip_quote_push() {
    // Bind a user function, then quote-push and call it
    // \a [a a +] = double  `double → pushes the Fun
    let src = "\\a [a a +] = double  `double";
    let orig = parse(src).expect("parse failed");
    let lowered = lower(&lift(&orig));

    // Op-level identity
    assert_eq!(orig.len(), lowered.len());

    // The last op in the lowered sequence should be Quote("double")
    let last = lowered.last().expect("non-empty ops");
    assert!(
        matches!(last, Op::Quote(w) if w.as_ref() == "double"),
        "last op should be Quote(double), got {last:?}"
    );

    // Eval: both should push a Fun onto the stack
    let r1 = run(&orig);
    let r2 = run(&lowered);
    assert_eq!(r1.len(), r2.len());
    assert!(
        matches!(r1[0], Value::Fun(_)),
        "orig: expected Fun on stack"
    );
    assert!(
        matches!(r2[0], Value::Fun(_)),
        "lowered: expected Fun on stack"
    );

    // Also verify the call works correctly after round-trip
    let src_call = "\\a [a a +] = double  5 `double !";
    let (rc1, rc2) = roundtrip(src_call);
    assert_real_eq(real(&rc1[0]), 10.0, "double(5) via orig");
    assert_real_eq(real(&rc2[0]), 10.0, "double(5) via lowered");
}

// ── Test 11: Form field access `{:x 42 :y 7} ,x` ─────────────────────────────

#[test]
fn roundtrip_form_field_access() {
    let src = "{:x 42 :y 7} ,x";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    assert_real_eq(real(&r1[0]), 42.0, ",x on form");
}

// ── Test 12: Reduce adverb `[1 2 3 4 5] +/` ──────────────────────────────────

#[test]
fn roundtrip_reduce_adverb() {
    let src = "[1 2 3 4 5] +/";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    assert_real_eq(real(&r1[0]), 15.0, "+/ reduce");

    // Verify the adverb round-trips through the graph
    let orig = parse(src).expect("parse failed");
    let lowered = lower(&lift(&orig));
    let has_adverb = lowered
        .iter()
        .any(|op| matches!(op, Op::Adverb(Adverb::Reduce)));
    assert!(has_adverb, "Adverb::Reduce should survive lift/lower");
}

// ── Test 13: Scan adverb `[1 2 3 4] +\\` ─────────────────────────────────────

#[test]
fn roundtrip_scan_adverb() {
    let src = "[1 2 3 4] +\\";
    let orig = parse(src).expect("parse failed");
    let lowered = lower(&lift(&orig));

    assert_eq!(orig.len(), lowered.len());

    let r1 = run(&orig);
    let r2 = run(&lowered);
    assert_eq!(r1.len(), r2.len());

    let has_scan = lowered
        .iter()
        .any(|op| matches!(op, Op::Adverb(Adverb::Scan)));
    assert!(has_scan, "Adverb::Scan should survive lift/lower");
}

// ── Test 14: Nested lambda closure `10 = x  \a [a x +] = addx  5 `addx !` ───

#[test]
fn roundtrip_lambda_closure() {
    // Lambda captures `x=10` from outer scope; addx(5) = 15
    let src = "10 = x  \\a [a x +] = addx  5 `addx !";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    assert_real_eq(real(&r1[0]), 15.0, "closure addx(5) = 15");
}

// ── Test 15: Each adverb `[1 2 3 4] @ neg` ───────────────────────────────────

#[test]
fn roundtrip_each_adverb() {
    let src = "[1 2 3 4] @ neg";
    let orig = parse(src).expect("parse failed");
    let lowered = lower(&lift(&orig));

    assert_eq!(orig.len(), lowered.len(), "op count preserved for @ neg");

    let r1 = run(&orig);
    let r2 = run(&lowered);
    assert_eq!(r1.len(), r2.len(), "@ neg should leave 1 value");
}

// ── Test 16: Bind-use-arithmetic `3 = a  4 = b  a b *` ──────────────────────

#[test]
fn roundtrip_bind_use_arithmetic() {
    let src = "3 = a  4 = b  a b *";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    assert_real_eq(real(&r1[0]), 12.0, "a*b where a=3, b=4");
}

// ── Test 17: Swap and subtract `10 3 swap -` ─────────────────────────────────

#[test]
fn roundtrip_swap_subtract() {
    let src = "10 3 swap -";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    // swap puts 3 on TOS, 10 below; `-` pops 3 and 10, pushes 3-10 = -7
    assert_real_eq(real(&r1[0]), -7.0, "10 3 swap -");
}

// ── Test 18: Over `2 3 over` ─────────────────────────────────────────────────

#[test]
fn roundtrip_over() {
    let src = "2 3 over";
    let (r1, r2) = roundtrip(src);
    assert_eq!(r1.len(), r2.len());
    // Should leave [2, 3, 2] — 3 values
    assert_eq!(r1.len(), 3);
    assert_real_eq(real(&r1[0]), 2.0, "over: bottom");
    assert_real_eq(real(&r1[1]), 3.0, "over: middle");
    assert_real_eq(real(&r1[2]), 2.0, "over: copy");
}

// ── Test 19: Graph node count for a complex program ─────────────────────────

#[test]
fn graph_node_count_complex_program() {
    // "1 2 3 = (a b c)  a b + c *"
    // Lit(1), Lit(2), Lit(3), BindMany, Word(a), Word(b), Word(+), Word(c), Word(*) = 9 nodes
    // Plus actually the parser emits Word nodes for variable references, not Lits
    let src = "1 2 3 = (a b c)  a b + c *";
    let orig = parse(src).expect("parse failed");
    let g = lift(&orig);
    // Node count should equal the number of ops
    assert_eq!(g.node_count(), orig.len(), "each op should become one node");
}

// ── Test 20: Topo-sort preserves semantics ────────────────────────────────────

#[test]
fn topo_sort_preserves_eval() {
    let src = "5 3 + 2 *";
    let orig = parse(src).expect("parse failed");
    let g = lift(&orig);
    let sorted_ids = topo_sort(&g);
    let lowered_topo = stax_graph::lower_ordered(&g, sorted_ids.into_iter());

    let r1 = run(&orig);
    let r2 = run(&lowered_topo);

    assert_eq!(r1.len(), r2.len());
    assert_real_eq(real(&r1[0]), 16.0, "topo-sorted 5+3)*2");
    assert_real_eq(real(&r1[0]), real(&r2[0]), "topo sort semantic equivalence");
}

// ── Test 21: SAPF `drop` semantics — `n list drop` drops first n elements ─────
//
// In SAPF/stax, `drop` is NOT a stack operator — it takes (n, stream) and
// drops the first n elements of the stream. Stack manipulation uses `nip`, `swap`,
// `dup`, `over` etc. This test verifies that `drop` works on streams.

#[test]
fn roundtrip_drop_over() {
    // `[1 2 3 4 5] 2 drop` → stream [3, 4, 5]; take 3 → [3, 4, 5]
    // Verify that `drop` on a list stream round-trips semantically
    let src = "[1 2 3 4 5] 2 drop";
    let orig = parse(src).expect("parse failed");
    let lowered = lower(&lift(&orig));
    assert_eq!(orig.len(), lowered.len(), "op count preserved for drop");

    let r1 = run(&orig);
    let r2 = run(&lowered);
    assert_eq!(r1.len(), r2.len(), "drop should leave 1 stream on stack");
    // Both should be streams
    assert!(
        matches!(r1[0], Value::Stream(_)),
        "drop result should be a Stream"
    );
    assert!(
        matches!(r2[0], Value::Stream(_)),
        "lowered drop result should be a Stream"
    );

    // Verify `over` (stack duplication) independently: `2 3 over` → [2, 3, 2]
    let src2 = "2 3 over";
    let orig2 = parse(src2).expect("parse failed");
    let lowered2 = lower(&lift(&orig2));
    let r_orig = run(&orig2);
    let r_low = run(&lowered2);
    assert_eq!(r_orig.len(), 3, "over should produce 3 values");
    assert_eq!(r_low.len(), 3, "lowered over should produce 3 values");
    assert_real_eq(real(&r_orig[0]), 2.0, "over: bottom");
    assert_real_eq(real(&r_orig[1]), 3.0, "over: mid");
    assert_real_eq(real(&r_orig[2]), 2.0, "over: copy");
    assert_real_eq(real(&r_low[0]), real(&r_orig[0]), "over round-trip: bottom");
    assert_real_eq(real(&r_low[1]), real(&r_orig[1]), "over round-trip: mid");
    assert_real_eq(real(&r_low[2]), real(&r_orig[2]), "over round-trip: copy");
}

// ── Test 22: Chained lambdas and calls ───────────────────────────────────────

#[test]
fn roundtrip_chained_lambdas() {
    // `square(x) = x*x`, `cube(x) = x * square(x)`, cube(3) = 27
    let src = "\\x [x x *] = square  \\n [n `square ! n *] = cube  3 `cube !";
    assert_roundtrip_reals(src);

    let (r1, _) = roundtrip(src);
    assert_eq!(r1.len(), 1);
    assert_real_eq(real(&r1[0]), 27.0, "cube(3)");
}

// ── Test 23: Mixed numeric and stream on stack ────────────────────────────────

#[test]
fn roundtrip_mixed_stack_types() {
    // Leave a Real and a Stream on the stack; both should survive round-trip.
    let src = "42  [1 2 3]";
    let orig = parse(src).expect("parse failed");
    let lowered = lower(&lift(&orig));

    let r1 = run(&orig);
    let r2 = run(&lowered);

    assert_eq!(r1.len(), 2, "should have Real and Stream on stack");
    assert_eq!(r2.len(), 2);

    assert!(matches!(r1[0], Value::Real(_)));
    assert!(matches!(r2[0], Value::Real(_)));
    assert_real_eq(real(&r1[0]), 42.0, "Real survives round-trip");

    assert!(matches!(r1[1], Value::Stream(_)));
    assert!(matches!(r2[1], Value::Stream(_)));
}

// ── Test 24: Graph edges represent data flow correctly ───────────────────────

#[test]
fn graph_edges_data_flow() {
    // "2 3 + 4 *" → 4 edges: Lit(2)→+, Lit(3)→+, +(out)→*, Lit(4)→*
    let src = "2 3 + 4 *";
    let g = lift(&parse(src).expect("parse failed"));

    let plus_id = g
        .nodes_in_order()
        .find(|n| matches!(&n.kind, NodeKind::Word(w) if w.as_ref() == "+"))
        .map(|n| n.id)
        .unwrap();

    let mul_id = g
        .nodes_in_order()
        .find(|n| matches!(&n.kind, NodeKind::Word(w) if w.as_ref() == "*"))
        .map(|n| n.id)
        .unwrap();

    // + should have 2 predecessors (Lit(2) and Lit(3))
    let plus_preds = g.predecessors(plus_id);
    assert_eq!(plus_preds.len(), 2, "+ should have 2 predecessors");

    // * should have 2 predecessors (+ and Lit(4))
    let mul_preds = g.predecessors(mul_id);
    assert_eq!(mul_preds.len(), 2, "* should have 2 predecessors");

    // + should have * as its only successor
    let plus_succs = g.successors(plus_id);
    assert_eq!(plus_succs.len(), 1, "+ should have 1 successor");
    assert_eq!(plus_succs[0], mul_id, "+ successor should be *");
}

// ── Test 25: Op-level identity for all round-tripped programs ─────────────────

#[test]
fn op_identity_all_programs() {
    let programs = [
        "1 2 3 + +",
        "5 dup *",
        "3 4 swap - neg",
        "[1 2 3] size",
        "42 = answer  answer",
        "'mysym",
        "`dup",
        "{:a 1 :b 2}",
        "\\x y [x y +] = add  1 2 `add !",
        "[1 2 3 4 5] +/",
    ];

    for src in programs {
        let orig = parse(src).expect("parse failed");
        let lowered = lower(&lift(&orig));
        assert_eq!(
            orig.len(),
            lowered.len(),
            "op count mismatch for `{src}`: {} vs {}",
            orig.len(),
            lowered.len()
        );
    }
}
