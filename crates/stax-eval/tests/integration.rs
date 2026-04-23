// End-to-end integration tests: parse + execute complex stax programs.
// Each test runs parse() → Interp::exec() and inspects the resulting stack.

use stax_core::Value;
use stax_eval::interp::{Interp, collect_to_vec};
use stax_parser::parse;

// ---- helpers ---------------------------------------------------------------

fn run(src: &str) -> Interp {
    let ops = parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e:?}"));
    let mut interp = Interp::new();
    interp.exec(&ops).unwrap_or_else(|e| panic!("exec failed for {src:?}: {e:?}"));
    interp
}

fn run_err(src: &str) -> stax_core::Error {
    let ops = parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e:?}"));
    let mut interp = Interp::new();
    interp.exec(&ops).expect_err("expected error")
}

fn top_real(src: &str) -> f64 {
    let mut interp = run(src);
    let v = interp.stack.pop().expect("empty stack");
    v.as_real().unwrap_or_else(|| panic!("top is not Real: {v:?}"))
}

fn top_list(src: &str) -> Vec<f64> {
    let mut interp = run(src);
    let v = interp.stack.pop().expect("empty stack");
    collect_to_vec(&v)
        .expect("not a stream")
        .into_iter()
        .map(|x| x.as_real().expect("list element not Real"))
        .collect()
}

// ===========================================================================
// 1. Basic arithmetic (sanity)
// ===========================================================================

#[test]
fn arith_add() {
    assert!((top_real("2 3 +") - 5.0).abs() < 1e-12);
}

#[test]
fn arith_chain() {
    // 1 2 3 + + = 6
    assert!((top_real("1 2 3 + +") - 6.0).abs() < 1e-12);
}

// ===========================================================================
// 2. Map-reduce: square each element, sum
// ===========================================================================

#[test]
fn map_reduce_squares() {
    // SAPF each pattern: list @ \x [body] ! — @ sets each mode, ! applies lambda to each element
    // [1 2 3 4 5] @ \x [x x *] !  →  [1 4 9 16 25], then +/ → 55
    let result = top_real("[1 2 3 4 5] @ \\x [x x *] ! +/");
    assert!((result - 55.0).abs() < 1e-12);
}

// ===========================================================================
// 3. Closure capture / adder
// ===========================================================================

// Note: In stax, calling a named Fun word auto-applies it. So `10 adder !`
// auto-calls adder (getting the inner Fun) then `!` tries to re-call it with
// an empty stack → Arity error. The `!` is only needed when using a quoted
// reference (`adder). The correct curried-adder idiom uses a bound name and
// the word auto-dispatches without `!`. The `!` is for explicitly stored funs
// (e.g. via Quote). Below we test a simpler closure capture: a function that
// adds a closed-over constant to its argument.

#[test]
fn closure_capture_constant() {
    // \x [x 100 +] = add100   42 add100   →  142
    // 'add100 Word auto-calls the Fun; no explicit ! needed
    let result = top_real("\\x [x 100 +] = add100  42 add100");
    assert!((result - 142.0).abs() < 1e-12);
}

#[test]
fn closure_adder_with_quote() {
    // Use ` (quote) to prevent auto-call, then explicit ! to apply.
    // \x [x 10 +] = offset10
    // 5 `offset10 !  →  15
    let result = top_real("\\x [x 10 +] = offset10  5 `offset10 !");
    assert!((result - 15.0).abs() < 1e-12);
}

// ===========================================================================
// 4. Deep pipeline: 1 to 5, sum, square
// ===========================================================================

#[test]
fn pipeline_sum_then_square() {
    // 1 5 to +/ dup *
    // 1+2+3+4+5 = 15; 15*15 = 225
    let result = top_real("1 5 to +/ dup *");
    assert!((result - 225.0).abs() < 1e-12);
}

// ===========================================================================
// 5. Multi-bind (tuple mode)
// ===========================================================================

#[test]
fn multi_bind_three_values() {
    // 1 2 3 = (a b c)  a b c + +  →  6
    let result = top_real("1 2 3 = (a b c)  a b c + +");
    assert!((result - 6.0).abs() < 1e-12);
}

// ===========================================================================
// 6. List destructuring
// ===========================================================================

#[test]
fn list_destructure_three() {
    // [10 20 30] = [x y z]  x y + z +  →  60
    let result = top_real("[10 20 30] = [x y z]  x y + z +");
    assert!((result - 60.0).abs() < 1e-12);
}

// ===========================================================================
// 7. Sorting
// ===========================================================================

#[test]
fn sort_ascending() {
    let items = top_list("[3 1 4 1 5 9 2 6] sort");
    assert_eq!(items, vec![1.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 9.0]);
}

#[test]
fn sort_descending() {
    let items = top_list("[3 1 4 1 5 9 2 6] sort>");
    assert_eq!(items, vec![9.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 1.0]);
}

// ===========================================================================
// 8. Cycling
// ===========================================================================

#[test]
fn cycle_and_take_n() {
    // 1 3 to cyc 6 N  →  [1 2 3 1 2 3]
    let items = top_list("1 3 to cyc 6 N");
    assert_eq!(items, vec![1.0, 2.0, 3.0, 1.0, 2.0, 3.0]);
}

// ===========================================================================
// 9. Form as record
// ===========================================================================

#[test]
fn form_get_name() {
    // {:name "hello" :value 42}  ,name  →  "hello"
    let mut interp = run("{:name \"hello\" :value 42} ,name");
    let v = interp.stack.pop().unwrap();
    assert!(matches!(&v, Value::Str(s) if s.as_ref() == "hello"));
}

#[test]
fn form_get_value() {
    // {:name "hello" :value 42}  ,value  →  42.0
    let result = top_real("{:name \"hello\" :value 42} ,value");
    assert!((result - 42.0).abs() < 1e-12);
}

// ===========================================================================
// 10. if/conditional (ternary: cond true-thunk false-thunk if)
// ===========================================================================

#[test]
fn if_true_branch() {
    // 1 [\[42]] [\[99]] if  → 42
    let result = top_real("1 \\[42] \\[99] if");
    assert!((result - 42.0).abs() < 1e-12);
}

#[test]
fn if_false_branch() {
    // 0 [\[42]] [\[99]] if  → 99
    let result = top_real("0 \\[42] \\[99] if");
    assert!((result - 99.0).abs() < 1e-12);
}

// ===========================================================================
// 11. Reduce: sum, product
// ===========================================================================

#[test]
fn reduce_sum() {
    // [1 2 3 4 5] +/  →  15
    let result = top_real("[1 2 3 4 5] +/");
    assert!((result - 15.0).abs() < 1e-12);
}

#[test]
fn reduce_product() {
    // [1 2 3 4 5] */  →  120
    let result = top_real("[1 2 3 4 5] */");
    assert!((result - 120.0).abs() < 1e-12);
}

// ===========================================================================
// 12. Scan
// ===========================================================================

#[test]
fn scan_running_sum() {
    // [1 2 3 4] +\  →  [1 3 6 10]
    let items = top_list("[1 2 3 4] +\\");
    assert_eq!(items, vec![1.0, 3.0, 6.0, 10.0]);
}

// ===========================================================================
// 13. Pairwise differences
// ===========================================================================

#[test]
fn pairwise_diff() {
    // [1 3 6 10] -^  →  [1 2 3 4]
    let items = top_list("[1 3 6 10] -^");
    assert_eq!(items, vec![1.0, 2.0, 3.0, 4.0]);
}

// ===========================================================================
// 14. Stack ops
// ===========================================================================

#[test]
fn stack_dup() {
    // 5 dup *  →  25
    let result = top_real("5 dup *");
    assert!((result - 25.0).abs() < 1e-12);
}

#[test]
fn stack_swap() {
    // 10 3 swap /  →  0.3 (10/3... wait: 3 10 swap/ = 10/3? No: swap puts 10 under 3)
    // "10 3 swap" → stack: 3 10 (bottom=3, top=10) → "/" pops b=top=10, a=3, = 3/10 = 0.3
    // Actually: pop b=10, a=3, result=a/b=3/10=0.3
    let result = top_real("10 3 swap /");
    assert!((result - 0.3).abs() < 1e-9);
}

#[test]
fn stack_over() {
    // 10 3 over  →  stack is: 10 3 10 (over copies second item to top)
    // Then + → 13, then + → 10+13 = wait, only 3 items: 10 3 10
    // "over" does: pop b=3, pop a=10, push a=10, push b=3, push a=10 → stack: 10 3 10
    // Then stack has 3 items; let's just check top = 10
    let mut interp = run("10 3 over");
    assert_eq!(interp.stack.len(), 3);
    let top = interp.stack.pop().unwrap().as_real().unwrap();
    assert!((top - 10.0).abs() < 1e-12);
}

// ===========================================================================
// 15. Fibonacci stream (using fib builtin)
// ===========================================================================

#[test]
fn fib_stream_first_10() {
    // fib starts at 0: [0 1 1 2 3 5 8 13 21 34 ...]
    let items = top_list("fib 10 N");
    assert_eq!(items, vec![0.0, 1.0, 1.0, 2.0, 3.0, 5.0, 8.0, 13.0, 21.0, 34.0]);
}

// ===========================================================================
// 16. Range and indexing
// ===========================================================================

#[test]
fn range_to_with_at() {
    // 0 4 to 2 at  →  2.0 (0-indexed)
    let result = top_real("0 4 to 2 at");
    assert!((result - 2.0).abs() < 1e-12);
}

#[test]
fn range_size() {
    // 1 10 to size  →  10
    let result = top_real("1 10 to size");
    assert!((result - 10.0).abs() < 1e-12);
}

// ===========================================================================
// 17. Reverse
// ===========================================================================

#[test]
fn reverse_list() {
    let items = top_list("[1 2 3 4 5] reverse");
    assert_eq!(items, vec![5.0, 4.0, 3.0, 2.0, 1.0]);
}

// ===========================================================================
// 18. Drop and take
// ===========================================================================

#[test]
fn drop_first_two() {
    let items = top_list("[10 20 30 40 50] 2 drop");
    assert_eq!(items, vec![30.0, 40.0, 50.0]);
}

#[test]
fn take_first_three() {
    let items = top_list("[10 20 30 40 50] 3 take");
    assert_eq!(items, vec![10.0, 20.0, 30.0]);
}

// ===========================================================================
// 19. Lambda with multiple values
// ===========================================================================

#[test]
fn lambda_sum_of_two() {
    // Note: "add" is a builtin (appends to list), so use a different name.
    // \a b [a b +] = mysum  3 7 mysum  → 10
    // The word auto-calls the Fun (no ! needed).
    let result = top_real("\\a b [a b +] = mysum  3 7 mysum");
    assert!((result - 10.0).abs() < 1e-12);
}

// ===========================================================================
// 20. Map over list (each)
// ===========================================================================

#[test]
fn map_increment_each() {
    // SAPF each: list @ word — @ pops list into each_list, next word is applied to each element
    // [1 2 3] @ inc  →  [2 3 4]
    let items = top_list("[1 2 3] @ inc");
    assert_eq!(items, vec![2.0, 3.0, 4.0]);
}

// ===========================================================================
// 21. Nested lambda / higher-order
// ===========================================================================

#[test]
fn higher_order_map_then_reduce() {
    // Double each element then sum: [1 2 3] @ \x [x 2 *] ! +/  →  2+4+6 = 12
    let result = top_real("[1 2 3] @ \\x [x 2 *] ! +/");
    assert!((result - 12.0).abs() < 1e-12);
}

// ===========================================================================
// 22. String value on stack
// ===========================================================================

#[test]
fn string_push_and_check() {
    let mut interp = run("\"hello world\"");
    let v = interp.stack.pop().unwrap();
    assert!(matches!(&v, Value::Str(s) if s.as_ref() == "hello world"));
}

// ===========================================================================
// 23. Sym value on stack
// ===========================================================================

#[test]
fn sym_push_and_check() {
    let mut interp = run("'mySymbol");
    let v = interp.stack.pop().unwrap();
    assert!(matches!(&v, Value::Sym(s) if s.as_ref() == "mySymbol"));
}

// ===========================================================================
// 24. Stack underflow error
// ===========================================================================

#[test]
fn stack_underflow_on_empty() {
    let err = run_err("+");
    assert!(matches!(err, stax_core::Error::StackUnderflow { .. }));
}

// ===========================================================================
// 25. Mirror operations
// ===========================================================================

#[test]
fn mirror0() {
    // [1 2 3 4 5] mirror0  →  [1 2 3 4 5 4 3 2 1]  (removes both endpoints from reflection)
    let items = top_list("[1 2 3 4 5] mirror0");
    assert_eq!(items, vec![1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 2.0]);
}

#[test]
fn mirror2() {
    // [1 2 3] mirror2  →  [1 2 3 3 2 1]
    let items = top_list("[1 2 3] mirror2");
    assert_eq!(items, vec![1.0, 2.0, 3.0, 3.0, 2.0, 1.0]);
}

// ===========================================================================
// 26. Clump
// ===========================================================================

#[test]
fn clump_pairs() {
    // [1 2 3 4 5 6] 2 clump  →  [[1 2] [3 4] [5 6]]
    let mut interp = run("[1 2 3 4 5 6] 2 clump");
    let outer = interp.stack.pop().unwrap();
    let rows = collect_to_vec(&outer).unwrap();
    assert_eq!(rows.len(), 3);
    let row0 = collect_to_vec(&rows[0]).unwrap();
    assert_eq!(row0[0].as_real().unwrap(), 1.0);
    assert_eq!(row0[1].as_real().unwrap(), 2.0);
}

// ===========================================================================
// 27. not / logic
// ===========================================================================

#[test]
fn not_of_zero() {
    let result = top_real("0 not");
    assert!((result - 1.0).abs() < 1e-12);
}

#[test]
fn not_of_nonzero() {
    let result = top_real("5 not");
    assert!((result - 0.0).abs() < 1e-12);
}

// ===========================================================================
// 28. 2ple / un2
// ===========================================================================

#[test]
fn tuple_2ple_then_un2() {
    // 10 20 2ple  →  [10 20]
    // then un2  →  pushes 10, 20 back
    let mut interp = run("10 20 2ple un2");
    assert_eq!(interp.stack.len(), 2);
    let b = interp.stack.pop().unwrap().as_real().unwrap();
    let a = interp.stack.pop().unwrap().as_real().unwrap();
    assert!((a - 10.0).abs() < 1e-12);
    assert!((b - 20.0).abs() < 1e-12);
}

// ===========================================================================
// 29. Form with has / keys
// ===========================================================================

#[test]
fn form_has_key() {
    // {:x 1} 'x has  →  1.0 (true)
    let result = top_real("{:x 1} 'x has");
    assert!((result - 1.0).abs() < 1e-12);
}

#[test]
fn form_missing_key() {
    // {:x 1} 'y has  →  0.0 (false)
    let result = top_real("{:x 1} 'y has");
    assert!((result - 0.0).abs() < 1e-12);
}

// ===========================================================================
// 30. at (zero-based indexing)
// ===========================================================================

#[test]
fn at_first_element() {
    // [10 20 30] 0 at  →  10
    let result = top_real("[10 20 30] 0 at");
    assert!((result - 10.0).abs() < 1e-12);
}

#[test]
fn at_last_element() {
    // [10 20 30] 2 at  →  30
    let result = top_real("[10 20 30] 2 at");
    assert!((result - 30.0).abs() < 1e-12);
}

#[test]
fn at_out_of_bounds() {
    // [10 20 30] 5 at  →  0.0 (at_zero returns 0 for out-of-bounds)
    let result = top_real("[10 20 30] 5 at");
    assert!((result - 0.0).abs() < 1e-12);
}
