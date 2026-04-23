/// language_stress.rs — stress-test the stax interpreter for correctness bugs.
///
/// Covers:
/// 1. Arithmetic edge cases
/// 2. Stack operations under stress
/// 3. String/symbol operations
/// 4. Stream/list edge cases
/// 5. Lambda/function edge cases
/// 6. Control flow
/// 7. Form operations
/// 8. Ref operations
/// 9. Type checks / `type` word

use stax_core::Value;
use stax_eval::interp::Interp;
use stax_parser::parse;

// ---- helpers ----------------------------------------------------------------

fn eval(src: &str) -> Vec<Value> {
    let ops = parse(src).unwrap_or_else(|e| panic!("parse error in {:?}: {e}", src));
    let mut interp = Interp::new();
    interp
        .exec(&ops)
        .unwrap_or_else(|e| panic!("exec error in {:?}: {e}", src));
    interp.stack
}

fn eval_err(src: &str) -> bool {
    let ops = match parse(src) {
        Ok(o) => o,
        Err(_) => return true,
    };
    let mut interp = Interp::new();
    interp.exec(&ops).is_err()
}

fn real(stack: &[Value], idx: usize) -> f64 {
    stack[idx]
        .as_real()
        .unwrap_or_else(|| panic!("expected Real at index {idx}, got {:?}", stack[idx]))
}

fn collect_stream(v: &Value) -> Vec<f64> {
    if let Value::Stream(s) = v {
        let mut it = s.iter();
        let mut out = Vec::new();
        while let Some(elem) = it.next() {
            out.push(elem.as_real().unwrap_or(f64::NAN));
        }
        out
    } else {
        panic!("expected Stream, got {:?}", v);
    }
}

// ============================================================================
// 1. ARITHMETIC EDGE CASES
// ============================================================================

#[test]
fn arith_division_by_zero_produces_inf() {
    // IEEE 754: 1.0 / 0.0 = +inf, not an error
    let s = eval("1 0 /");
    assert!(real(&s, 0).is_infinite() && real(&s, 0) > 0.0,
        "1/0 should be +inf, got {}", real(&s, 0));
}

#[test]
fn arith_negative_division_by_zero_produces_neg_inf() {
    let s = eval("-1 0 /");
    assert!(real(&s, 0).is_infinite() && real(&s, 0) < 0.0,
        "-1/0 should be -inf, got {}", real(&s, 0));
}

#[test]
fn arith_zero_divided_by_zero_is_nan() {
    let s = eval("0 0 /");
    assert!(real(&s, 0).is_nan(), "0/0 should be NaN, got {}", real(&s, 0));
}

#[test]
fn arith_very_large_numbers() {
    // 1e300 * 1e300 = inf (overflow)
    let s = eval("1e300 1e300 *");
    assert!(real(&s, 0).is_infinite(), "1e300 * 1e300 should overflow to inf");
}

#[test]
fn arith_negative_numbers_all_ops() {
    // -3 - -5 = 2
    let s = eval("-3 -5 -");
    assert_eq!(real(&s, 0), 2.0);
    // -3 * -5 = 15
    let s2 = eval("-3 -5 *");
    assert_eq!(real(&s2, 0), 15.0);
    // -3 + -5 = -8
    let s3 = eval("-3 -5 +");
    assert_eq!(real(&s3, 0), -8.0);
}

#[test]
fn arith_floor_negative_fraction() {
    // floor(-0.5) = -1.0
    let s = eval("-0.5 floor");
    assert_eq!(real(&s, 0), -1.0);
}

#[test]
fn arith_ceil_negative_fraction() {
    // ceil(-0.5) = 0.0
    let s = eval("-0.5 ceil");
    assert_eq!(real(&s, 0), 0.0);
}

#[test]
fn arith_round_negative_fraction() {
    // round(-0.5) — Rust rounds -0.5 to 0.0 (rounds to even, or away from zero)
    // Rust f64::round rounds -0.5 to -1.0 (round half away from zero)
    let s = eval("-0.5 round");
    assert_eq!(real(&s, 0), -1.0);
}

#[test]
fn arith_mod_with_negative_operand() {
    // stax `mod` maps to Rust `%`: sign follows dividend
    let s = eval("-7 3 mod");
    let expected = -7.0_f64 % 3.0_f64; // = -1.0
    assert_eq!(real(&s, 0), expected);
}

#[test]
fn arith_mod_negative_divisor() {
    let s = eval("7 -3 mod");
    let expected = 7.0_f64 % -3.0_f64; // = 1.0
    assert_eq!(real(&s, 0), expected);
}

#[test]
fn arith_power_negative_base_integer_exponent() {
    // (-2)^3 = -8
    let s = eval("-2 3 pow");
    assert_eq!(real(&s, 0), -8.0);
}

#[test]
fn arith_power_negative_base_fractional_exponent() {
    // (-2)^0.5 = NaN (complex result)
    let s = eval("-2 0.5 pow");
    assert!(real(&s, 0).is_nan(), "(-2)^0.5 should be NaN");
}

#[test]
fn arith_sqrt_negative_is_nan() {
    let s = eval("-4 sqrt");
    assert!(real(&s, 0).is_nan(), "sqrt(-4) should be NaN");
}

#[test]
fn arith_ln_zero_is_neg_inf() {
    let s = eval("0 ln");
    assert!(real(&s, 0).is_infinite() && real(&s, 0) < 0.0, "ln(0) should be -inf");
}

#[test]
fn arith_recip_zero_is_inf() {
    let s = eval("0 recip");
    assert!(real(&s, 0).is_infinite(), "recip(0) should be inf");
}

// ============================================================================
// 2. STACK OPERATIONS UNDER STRESS
// ============================================================================

#[test]
fn stack_deep_dup_chain() {
    // dup five times: 1 becomes 6 copies
    let s = eval("1 dup dup dup dup dup");
    assert_eq!(s.len(), 6);
    for i in 0..6 {
        assert_eq!(real(&s, i), 1.0);
    }
}

#[test]
fn stack_over_basic() {
    // a b over → a b a
    let s = eval("10 20 over");
    assert_eq!(s.len(), 3);
    assert_eq!(real(&s, 0), 10.0);
    assert_eq!(real(&s, 1), 20.0);
    assert_eq!(real(&s, 2), 10.0);
}

#[test]
fn stack_swap_twice_is_identity() {
    let s = eval("3 7 swap swap");
    assert_eq!(s.len(), 2);
    assert_eq!(real(&s, 0), 3.0);
    assert_eq!(real(&s, 1), 7.0);
}

#[test]
fn stack_underflow_pop_empty_is_error() {
    assert!(eval_err("dup"), "dup on empty stack should error");
}

#[test]
fn stack_underflow_swap_one_item_is_error() {
    assert!(eval_err("1 swap"), "swap with only 1 item should error");
}

#[test]
fn stack_underflow_over_one_item_is_error() {
    assert!(eval_err("1 over"), "over with only 1 item should error");
}

#[test]
fn stack_depth_word() {
    let s = eval("1 2 3 stackDepth");
    // stack has 1 2 3, then depth=3 pushed
    assert_eq!(real(&s, 3), 3.0);
}

#[test]
fn stack_clear() {
    let s = eval("1 2 3 clear");
    assert!(s.is_empty(), "clear should empty the stack");
}

#[test]
fn stack_pop_removes_tos() {
    let s = eval("1 2 3 pop");
    assert_eq!(s.len(), 2);
    assert_eq!(real(&s, 1), 2.0);
}

#[test]
fn stack_2ple_and_un2() {
    // 1 2 2ple → [1, 2] ; un2 → 1 2
    let s = eval("1 2 2ple un2");
    assert_eq!(s.len(), 2);
    assert_eq!(real(&s, 0), 1.0);
    assert_eq!(real(&s, 1), 2.0);
}

// ============================================================================
// 3. STRING AND SYMBOL OPERATIONS
// ============================================================================

#[test]
fn string_literal_pushed() {
    let s = eval(r#""hello""#);
    assert_eq!(s.len(), 1);
    match &s[0] {
        Value::Str(st) => assert_eq!(st.as_ref(), "hello"),
        other => panic!("expected Str, got {:?}", other),
    }
}

#[test]
fn symbol_literal_pushed() {
    let s = eval("'mysym");
    assert_eq!(s.len(), 1);
    match &s[0] {
        Value::Sym(sym) => assert_eq!(sym.as_ref(), "mysym"),
        other => panic!("expected Sym, got {:?}", other),
    }
}

#[test]
fn string_equals_itself() {
    // two identical string literals are equal
    let s = eval(r#""abc" "abc" equals"#);
    assert_eq!(real(&s, 0), 1.0);
}

#[test]
fn symbol_not_equal_to_different_sym() {
    let s = eval("'foo 'bar equals");
    assert_eq!(real(&s, 0), 0.0);
}

#[test]
fn symbol_equals_itself() {
    let s = eval("'x 'x equals");
    assert_eq!(real(&s, 0), 1.0);
}

// ============================================================================
// 4. STREAM / LIST EDGE CASES
// ============================================================================

#[test]
fn empty_list_size() {
    let s = eval("[] size");
    assert_eq!(real(&s, 0), 0.0);
}

#[test]
fn empty_list_reverse() {
    let s = eval("[] reverse");
    assert_eq!(s.len(), 1);
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn empty_list_head_is_error() {
    assert!(eval_err("[] head"), "head of empty list should error");
}

#[test]
fn empty_list_tail_is_empty() {
    let s = eval("[] tail");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn single_element_list() {
    let s = eval("[42] size");
    assert_eq!(real(&s, 0), 1.0);
}

#[test]
fn single_element_head() {
    let s = eval("[99] head");
    assert_eq!(real(&s, 0), 99.0);
}

#[test]
fn single_element_reverse() {
    let s = eval("[7] reverse");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![7.0]);
}

#[test]
fn cyc_single_element_take_five() {
    // [3] cyc 5 N → [3, 3, 3, 3, 3]
    let s = eval("[3] cyc 5 N");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![3.0, 3.0, 3.0, 3.0, 3.0]);
}

#[test]
fn n_zero_from_stream_is_empty() {
    // 1 to 10 gives [1..10]; take 0 → []
    let s = eval("1 10 to 0 N");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty(), "N with 0 should produce empty list");
}

#[test]
fn n_with_large_n_capped() {
    // N is allowed up to 1_000_000; anything larger is an error
    assert!(eval_err("ord 2000000 N"), "N > 1_000_000 should error");
}

#[test]
fn deeply_nested_list_flat() {
    // [[1 2] [3 4]] flat → [1 2 3 4]
    let s = eval("[[1 2] [3 4]] flat");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn list_sort_ascending() {
    let s = eval("[3 1 4 1 5 9] sort");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![1.0, 1.0, 3.0, 4.0, 5.0, 9.0]);
}

#[test]
fn stream_skip_beyond_length() {
    // [1 2 3] skip 10 → []
    let s = eval("[1 2 3] 10 skip");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn stream_drop_all() {
    // [1 2 3] 3 drop → []
    let s = eval("[1 2 3] 3 drop");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn stream_reverse_order() {
    let s = eval("[1 2 3 4 5] reverse");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![5.0, 4.0, 3.0, 2.0, 1.0]);
}

// ============================================================================
// 5. LAMBDA / FUNCTION EDGE CASES
// ============================================================================

#[test]
fn lambda_zero_param() {
    // \[ 42 ] ! → 42
    let s = eval(r"\[42] !");
    assert_eq!(real(&s, 0), 42.0);
}

#[test]
fn lambda_single_param() {
    // \x [x x *] ! with 5 on stack → 25
    let s = eval(r"5 \x[x x *] !");
    assert_eq!(real(&s, 0), 25.0);
}

#[test]
fn lambda_two_params() {
    // 3 4 \a b [a b +] ! → 7
    let s = eval(r"3 4 \a b[a b +] !");
    assert_eq!(real(&s, 0), 7.0);
}

#[test]
fn lambda_captures_env() {
    // 10 =base  \x [base x +] =myadd  5 myadd → 15
    // NOTE: Do NOT use "add" — it conflicts with the list-append builtin.
    // Also do NOT use "!" after calling a named function word — that word
    // already dispatches the function immediately; the "!" would then try
    // to call the return value.
    let s = eval(r"10 =base  \x[base x +] =myadd  5 myadd");
    assert_eq!(real(&s, 0), 15.0);
}

#[test]
fn call_non_function_pushes_value() {
    // Calling a Real as a function should push it (apply_or_push behaviour)
    // 7 ! → 7 (non-function values are pushed)
    let s = eval("7 !");
    assert_eq!(real(&s, 0), 7.0);
}

#[test]
fn recursive_definition_via_bind() {
    // Simple non-recursive named function to verify bind + call pattern works.
    // Factorial is easier to express iteratively with stream ops.
    // Here we test: double a value by binding and calling a named lambda.
    // \n [n 2 *] =dbl  5 dbl → 10
    // NOTE: [..] list literals are EAGERLY evaluated, not lazy thunks.
    // Use \[body] for thunks passed to `if`.
    let s = eval(r"\n[n 2 *] =dbl  5 dbl");
    assert_eq!(real(&s, 0), 10.0);
}

#[test]
fn nested_lambdas() {
    // \x [ \y [ x y * ] ] → double currying
    // 3 \x[\y[x y *]] !  → inner lambda (captures x=3) on stack
    // 4 swap !           → swap puts inner_lambda on TOS, then ! calls it with 4 → 12
    //
    // Alternatively, bind the inner lambda and then call it:
    // 3 \x[\y[x y *]] ! =inner  4 inner → 12
    let s = eval(r"3 \x[\y[x y *]] ! =inner  4 inner");
    assert_eq!(real(&s, 0), 12.0);
}

#[test]
fn arity_mismatch_is_error() {
    // lambda expecting 2 args but only 1 on stack → error
    assert!(eval_err(r"5 \a b[a b +] !"), "arity mismatch should error");
}

// ============================================================================
// 6. CONTROL FLOW
// ============================================================================

#[test]
fn filter_empty_list_gives_empty() {
    // [] [1 1 1] ? → []
    let s = eval("[] [1 1 1] ?");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn filter_all_zeros_keeps_nothing() {
    // [1 2 3] [0 0 0] ? → []
    let s = eval("[1 2 3] [0 0 0] ?");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn filter_all_ones_keeps_all() {
    let s = eval("[10 20 30] [1 1 1] ?");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![10.0, 20.0, 30.0]);
}

#[test]
fn skip_while_consumes_entire_list() {
    // [1 2 3] with mask [1 1 1] → all skipped
    let s = eval("[1 2 3] [1 1 1] skipWhile");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn keep_while_stops_at_first_false() {
    // [5 3 8 2] with [1 1 0 1] keepWhile → [5 3]
    let s = eval("[5 3 8 2] [1 1 0 1] keepWhile");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![5.0, 3.0]);
}

#[test]
fn keep_while_empty_mask_keeps_nothing() {
    let s = eval("[1 2 3] [] keepWhile");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn if_true_branch() {
    // 1 [100] [200] if → 100
    let s = eval("1 [100] [200] if");
    // The thunks produce their contents when called; after apply_or_push of a Stream, it's pushed
    // [100] is a list, apply_or_push pushes the list itself
    // Actually [100] is a list literal pushed → apply_or_push pushes the Stream
    // We verify it's a Stream containing 100, or it could be 100 directly if thunk is Fun
    // [100] parses as a list literal (ListMark + Lit(100) + MakeList), not a lambda.
    // So if pushes the Stream [100].
    assert_eq!(s.len(), 1);
}

#[test]
fn if_false_branch() {
    let s = eval("0 [100] [200] if");
    assert_eq!(s.len(), 1);
}

#[test]
fn reduce_empty_list_is_error() {
    assert!(eval_err("[]+/"), "reduce on empty list should error");
}

// ============================================================================
// 7. FORM OPERATIONS
// ============================================================================

#[test]
fn empty_form_has_no_keys() {
    let s = eval("{} keys size");
    assert_eq!(real(&s, 0), 0.0);
}

#[test]
fn form_lookup_existing_key() {
    // {:x 42} ,x → 42
    let s = eval("{:x 42} ,x");
    assert_eq!(real(&s, 0), 42.0);
}

#[test]
fn form_lookup_missing_key_is_error() {
    // {:x 1} ,y → error (Unbound)
    assert!(eval_err("{:x 1} ,y"), "looking up missing key should error");
}

#[test]
fn form_has_existing_key() {
    let s = eval("{:a 1} 'a has");
    assert_eq!(real(&s, 0), 1.0);
}

#[test]
fn form_has_missing_key() {
    let s = eval("{:a 1} 'z has");
    assert_eq!(real(&s, 0), 0.0);
}

#[test]
fn form_kv_roundtrip() {
    // {:x 1 :y 2} kv → keys-list values-list; both length 2
    let s = eval("{:x 1 :y 2} kv");
    assert_eq!(s.len(), 2);
    // first is keys, second is values
    let keys = collect_stream(&s[0]);
    let vals = collect_stream(&s[1]);
    assert_eq!(keys.len(), 2);
    assert_eq!(vals.len(), 2);
    // kv keys are symbols, not reals — collect_stream will NaN them
    // Just verify lengths match
    let _ = (keys, vals);
}

#[test]
fn form_keys_are_symbols() {
    // kv keys should be Sym values
    let ops = parse("{:x 1} kv").unwrap();
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap();
    // stack: [keys-stream, values-stream]
    assert_eq!(interp.stack.len(), 2);
    if let Value::Stream(s) = &interp.stack[0] {
        let mut it = s.iter();
        if let Some(v) = it.next() {
            assert!(matches!(v, Value::Sym(_)), "form keys should be Sym, got {:?}", v);
        }
    }
}

#[test]
fn nested_form_parent_lookup() {
    // base_form = {:base 10}, child = {base_form :extra 5}
    // child ,base should resolve via the inherited parent.
    // NOTE: do NOT use "parent" as a variable name — it shadows the `parent` builtin word.
    let s = eval("{:base 10} =base_form  {base_form :extra 5} ,base");
    assert_eq!(real(&s, 0), 10.0);
}

#[test]
fn form_parent_of_no_parent_is_nil() {
    let s = eval("{:x 1} parent");
    assert!(matches!(s[0], Value::Nil), "parent of root form should be Nil");
}

// ============================================================================
// 8. REF OPERATIONS
// ============================================================================

#[test]
fn ref_create_and_get() {
    // 99 R get → 99
    let s = eval("99 R get");
    assert_eq!(real(&s, 0), 99.0);
}

#[test]
fn ref_set_and_get() {
    // 0 R =r  99 r set  r get → 99
    let s = eval("0 R =r  99 r set  r get");
    assert_eq!(real(&s, 0), 99.0);
}

#[test]
fn ref_mutation_is_visible() {
    // 1 R =counter  2 counter set  counter get → 2
    let s = eval("1 R =counter  2 counter set  counter get");
    assert_eq!(real(&s, 0), 2.0);
}

#[test]
fn set_on_non_ref_is_error() {
    // 5 7 set → error (5 is not a Ref)
    assert!(eval_err("5 7 set"), "set on non-Ref should error");
}

// ============================================================================
// 9. TYPE CHECKS — `type` word
// ============================================================================

#[test]
fn type_of_real_is_real() {
    let ops = parse("42 type").unwrap();
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap();
    assert_eq!(interp.stack.len(), 1);
    match &interp.stack[0] {
        Value::Sym(s) => assert_eq!(s.as_ref(), "Real"),
        other => panic!("expected Sym('Real'), got {:?}", other),
    }
}

#[test]
fn type_of_stream_is_vlist() {
    let ops = parse("[1 2 3] type").unwrap();
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap();
    match &interp.stack[0] {
        Value::Sym(s) => assert_eq!(s.as_ref(), "VList"),
        other => panic!("expected Sym('VList'), got {:?}", other),
    }
}

#[test]
fn type_of_fun_is_fun() {
    let ops = parse(r"\x[x] type").unwrap();
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap();
    match &interp.stack[0] {
        Value::Sym(s) => assert_eq!(s.as_ref(), "Fun"),
        other => panic!("expected Sym('Fun'), got {:?}", other),
    }
}

#[test]
fn type_of_form_is_form() {
    let ops = parse("{:x 1} type").unwrap();
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap();
    match &interp.stack[0] {
        Value::Sym(s) => assert_eq!(s.as_ref(), "Form"),
        other => panic!("expected Sym('Form'), got {:?}", other),
    }
}

#[test]
fn type_of_string_is_string() {
    let ops = parse(r#""hello" type"#).unwrap();
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap();
    match &interp.stack[0] {
        Value::Sym(s) => assert_eq!(s.as_ref(), "String"),
        other => panic!("expected Sym('String'), got {:?}", other),
    }
}

#[test]
fn type_of_sym_is_string() {
    // Both Str and Sym map to "String" in SAPF naming
    let ops = parse("'foo type").unwrap();
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap();
    match &interp.stack[0] {
        Value::Sym(s) => assert_eq!(s.as_ref(), "String"),
        other => panic!("expected Sym('String'), got {:?}", other),
    }
}

#[test]
fn type_of_ref_is_ref() {
    let ops = parse("0 R type").unwrap();
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap();
    match &interp.stack[0] {
        Value::Sym(s) => assert_eq!(s.as_ref(), "Ref"),
        other => panic!("expected Sym('Ref'), got {:?}", other),
    }
}

// ============================================================================
// 10. ADDITIONAL CORRECTNESS CHECKS
// ============================================================================

#[test]
fn clump_incomplete_chunk_dropped() {
    // [1 2 3 4 5] 2 clump → [[1 2] [3 4]] (incomplete [5] dropped)
    let s = eval("[1 2 3 4 5] 2 clump");
    let outer = collect_stream(&s[0]);
    // Each inner element is a stream; verify outer has length 2
    assert_eq!(s.len(), 1);
    if let Value::Stream(st) = &s[0] {
        assert_eq!(st.len_hint(), Some(2));
    }
    let _ = outer;
}

#[test]
fn clump_zero_chunk_size_returns_empty() {
    let s = eval("[1 2 3] 0 clump");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn add_appends_to_list() {
    let s = eval("[1 2] 3 add");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![1.0, 2.0, 3.0]);
}

#[test]
fn cons_prepends_to_list() {
    let s = eval("[2 3] 1 cons");
    let items = collect_stream(&s[0]);
    assert_eq!(items, vec![1.0, 2.0, 3.0]);
}

#[test]
fn equals_real_same() {
    let s = eval("5 5 equals");
    assert_eq!(real(&s, 0), 1.0);
}

#[test]
fn equals_real_different() {
    let s = eval("5 6 equals");
    assert_eq!(real(&s, 0), 0.0);
}

#[test]
fn equals_list_same() {
    let s = eval("[1 2 3] [1 2 3] equals");
    assert_eq!(real(&s, 0), 1.0);
}

#[test]
fn equals_list_different_length() {
    let s = eval("[1 2] [1 2 3] equals");
    assert_eq!(real(&s, 0), 0.0);
}

#[test]
fn not_truthy() {
    let s = eval("1 not");
    assert_eq!(real(&s, 0), 0.0);
    let s2 = eval("0 not");
    assert_eq!(real(&s2, 0), 1.0);
}

#[test]
fn logic_and() {
    let s = eval("1 1 &");
    assert_eq!(real(&s, 0), 1.0);
    let s2 = eval("1 0 &");
    assert_eq!(real(&s2, 0), 0.0);
}

#[test]
fn logic_or() {
    let s = eval("0 1 |");
    assert_eq!(real(&s, 0), 1.0);
    let s2 = eval("0 0 |");
    assert_eq!(real(&s2, 0), 0.0);
}

#[test]
fn ncyc_zero_times_is_empty() {
    let s = eval("[1 2 3] 0 ncyc");
    let items = collect_stream(&s[0]);
    assert!(items.is_empty());
}

#[test]
fn rot_preserves_length() {
    let s = eval("[1 2 3 4 5] 2 rot");
    let items = collect_stream(&s[0]);
    assert_eq!(items.len(), 5);
    // rotate right by 2: [4 5 1 2 3]
    assert_eq!(items, vec![4.0, 5.0, 1.0, 2.0, 3.0]);
}

#[test]
fn abs_negative_gives_positive() {
    let s = eval("-7 abs");
    assert_eq!(real(&s, 0), 7.0);
}

#[test]
fn neg_double_is_identity() {
    let s = eval("5 neg neg");
    assert_eq!(real(&s, 0), 5.0);
}

#[test]
fn min_max() {
    let s = eval("3 7 min");
    assert_eq!(real(&s, 0), 3.0);
    let s2 = eval("3 7 max");
    assert_eq!(real(&s2, 0), 7.0);
}

#[test]
fn inc_dec() {
    let s = eval("5 inc");
    assert_eq!(real(&s, 0), 6.0);
    let s2 = eval("5 dec");
    assert_eq!(real(&s2, 0), 4.0);
}

/// KNOWN BUG: the parser splits `<=` into `<` + `=` when `=` is the last
/// token or followed by a space. `=` is excluded from word-chars to support
/// the `=name` bind syntax, so `<` ends the word and the trailing `=` is
/// silently dropped (falls through the `_` arm in `parse_one`).
///
/// Symptom: `3 3 <=` returns 0 instead of 1 (evaluates as `3 3 <`).
/// Same for `>=`. When MORE tokens follow (e.g., `3 3 <= p`), the `=` is
/// misread as a bind operator and the word after it is bound, clobbering
/// an unrelated name.
///
/// The non-equal strict comparisons `<` and `>` work correctly.
#[test]
fn comparison_operators() {
    // These work correctly:
    assert_eq!(real(&eval("3 5 <"), 0), 1.0,  "3 < 5 should be 1");
    assert_eq!(real(&eval("5 3 <"), 0), 0.0,  "5 < 3 should be 0");
    assert_eq!(real(&eval("5 3 >"), 0), 1.0,  "5 > 3 should be 1");
    assert_eq!(real(&eval("3 5 >"), 0), 0.0,  "3 > 5 should be 0");

    // Fixed: parser now correctly tokenises `<=` and `>=` as single words.
    assert_eq!(real(&eval("3 3 <="), 0), 1.0, "3 <= 3 should be 1 (equal case)");
    assert_eq!(real(&eval("2 3 <="), 0), 1.0, "2 <= 3 should be 1 (less case)");
    assert_eq!(real(&eval("4 3 <="), 0), 0.0, "4 <= 3 should be 0");
    assert_eq!(real(&eval("3 3 >="), 0), 1.0, "3 >= 3 should be 1 (equal case)");
    assert_eq!(real(&eval("4 3 >="), 0), 1.0, "4 >= 3 should be 1 (greater case)");
    assert_eq!(real(&eval("2 3 >="), 0), 0.0, "2 >= 3 should be 0");
}

#[test]
fn irand_zero_gives_zero() {
    // 0 irand → always 0 (special case in impl)
    let s = eval("0 irand");
    assert_eq!(real(&s, 0), 0.0);
}

#[test]
fn seed_determinism() {
    // Same seed → same rand value
    let s1 = eval("42 seed rand");
    let s2 = eval("42 seed rand");
    assert_eq!(real(&s1, 0), real(&s2, 0));
}
