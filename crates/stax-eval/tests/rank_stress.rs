//! Stress tests for the rank-lifting and adverb system.
//!
//! Covers:
//!   1. Basic each (`@`) — unary and binary mapping, nested lists
//!   2. Each depth (`@@`, `@@@`) — nested list mapping
//!   3. Zip mode (two `@` calls) — parallel element iteration
//!   4. Outer products (`@1`, `@2`) — cartesian-product broadcasting
//!   5. Reduce (`+/`, `-/`, `*/`, `max/`) — finite and edge cases
//!   6. Scan (`+\`, `*\`) — running prefix ops
//!   7. Pairwise (`+^`, `-^`) — adjacent-pair ops
//!   8. Mixed — reduce of scan, scan then take, map then reduce
//!   9. Infinite stream adverbs — lazy `+\` and `+^` on `ord`/`nat`
//!  10. Error cases — reduce empty, adverb on non-list
//!  11. Nested rank — map then inner reduce via lambda
//!  12. Lambdas — map squaring, map with captured arg

use stax_core::Value;
use stax_eval::interp::Interp;
use stax_parser::parse;

// ---- helpers ----------------------------------------------------------------

fn try_eval(src: &str) -> Result<Vec<Value>, String> {
    let ops = parse(src).map_err(|e| format!("parse error in '{src}': {e}"))?;
    let mut interp = Interp::new();
    interp
        .exec(&ops)
        .map_err(|e| format!("exec error in '{src}': {e}"))?;
    Ok(interp.stack)
}

/// Run src and return the full stack on success.
fn eval(src: &str) -> Vec<Value> {
    try_eval(src).unwrap_or_else(|e| panic!("{e}"))
}

/// Pop the top value and assert it is truthy.
fn assert_truthy(src: &str) {
    let mut stack = eval(src);
    let v = stack
        .pop()
        .unwrap_or_else(|| panic!("empty stack after '{src}'"));
    assert!(v.is_truthy(), "expected truthy from '{src}', got: {v:?}");
}

/// Pop the top value and assert it equals the expected real.
fn assert_top_real(src: &str, expected: f64) {
    let mut stack = eval(src);
    let v = stack
        .pop()
        .unwrap_or_else(|| panic!("empty stack after '{src}'"));
    let r = v
        .as_real()
        .unwrap_or_else(|| panic!("top of '{src}' is not real, got: {v:?}"));
    assert!(
        (r - expected).abs() < 1e-9,
        "'{src}': expected {expected}, got {r}"
    );
}

/// Assert that evaluating src produces an error.
fn assert_err(src: &str) {
    assert!(
        try_eval(src).is_err(),
        "expected error from '{src}' but it succeeded"
    );
}

// =============================================================================
// 1. BASIC EACH (`@`) — unary map, binary with scalar, nested structure
// =============================================================================

/// Map `neg` (unary negate) over a list.
#[test]
fn each_unary_neg() {
    assert_truthy("[1 2 3] @ neg [-1 -2 -3] equals");
}

/// Map `abs` over a list with negative values.
#[test]
fn each_unary_abs() {
    assert_truthy("[-3 0 4 -1] @ abs [3 0 4 1] equals");
}

/// Map `sq` (square) over a list.
#[test]
fn each_unary_sq() {
    assert_truthy("[1 2 3 4 5] @ sq [1 4 9 16 25] equals");
}

/// Each with an extra scalar argument already on the stack (stack-preservation).
/// `100 [1 2 3] @ +` — 100 is below the mark; each element gets 100 added.
#[test]
fn each_binary_scalar_below_mark() {
    assert_truthy("100 [1 2 3] @ + [101 102 103] equals");
}

/// Each with a scalar pushed *after* the list pops (extra arg after @).
/// `[1 2 3] @ 10 +` — 10 is the extra arg pushed inside the each body.
#[test]
fn each_binary_scalar_extra_arg() {
    assert_truthy("[1 2 3] @ 10 + [11 12 13] equals");
}

/// Each with two base args below the mark.
#[test]
fn each_binary_two_base_args() {
    assert_truthy("1 2 [3 4 5] @ + [5 6 7] equals");
}

/// Map over a list of lists — each outer element (inner list) gets negated as a whole.
/// Because `neg` on a Stream maps through it, each inner stream ends up negated.
#[test]
fn each_over_list_of_lists_neg() {
    assert_truthy("[[1 2][3 4]] @ neg [[-1 -2][-3 -4]] equals");
}

/// Map `sqrt` to verify non-integer results.
#[test]
fn each_unary_sqrt() {
    assert_truthy("[4 9 16] @ sqrt [2 3 4] equals");
}

// =============================================================================
// 2. EACH DEPTH (`@@`, `@@@`) — nested list mapping
// =============================================================================

/// `@@` maps two levels deep through a list of lists.
#[test]
fn each_depth2_neg() {
    assert_truthy("[[1 2][3 4]] @@ neg [[-1 -2][-3 -4]] equals");
}

/// `@@` on a 2-deep nested structure mapping `sq`.
#[test]
fn each_depth2_sq() {
    assert_truthy("[[1 2][3 4]] @@ sq [[1 4][9 16]] equals");
}

/// `@@@` maps three levels deep.
#[test]
fn each_depth3_neg() {
    assert_truthy("[[[1 2][3 4]][[5 6][7 8]]] @@@ neg [[[-1 -2][-3 -4]][[-5 -6][-7 -8]]] equals");
}

/// `@@@` on a three-deep structure with `abs`.
#[test]
fn each_depth3_abs() {
    assert_truthy("[[[1 -2]][[3 -4]]] @@@ abs [[[1 2]][[3 4]]] equals");
}

// =============================================================================
// 3. ZIP MODE (two `@` calls) — element-wise application
// =============================================================================

/// Two `@` calls zip two equal-length lists, then apply `+`.
#[test]
fn zip_add_equal_length() {
    assert_truthy("[1 2 3] @ [10 20 30] @ + [11 22 33] equals");
}

/// Zip with `*` operator.
#[test]
fn zip_multiply() {
    assert_truthy("[2 3 4] @ [5 6 7] @ * [10 18 28] equals");
}

/// Zip with `-` operator.
#[test]
fn zip_subtract() {
    assert_truthy("[10 20 30] @ [1 2 3] @ - [9 18 27] equals");
}

/// Zip truncates to the shorter list when lengths differ.
#[test]
fn zip_unequal_lengths_truncates() {
    assert_truthy("[1 2 3] @ [10 20] @ + [11 22] equals");
}

/// Zip with a lambda via `!` — add corresponding elements and double.
#[test]
fn zip_lambda() {
    assert_truthy("[1 2 3] @ [10 20 30] @ \\a b [a b + 2 *] ! [22 44 66] equals");
}

// =============================================================================
// 4. OUTER PRODUCTS (`@1`, `@2`) — Cartesian broadcasting
// =============================================================================

/// `[1 2] @1 [10 20] @2 +` → outer product under addition.
#[test]
fn outer_product_add_2x2() {
    assert_truthy("[1 2] @1 [10 20] @2 + [[11 21][12 22]] equals");
}

/// `[1 2 3] @1 [10 20] @2 +` → 3-row × 2-col outer product.
#[test]
fn outer_product_add_3x2() {
    assert_truthy("[1 2 3] @1 [10 20] @2 + [[11 21][12 22][13 23]] equals");
}

/// `[1 2] @1 [10 20 30] @2 +` → 2-row × 3-col.
#[test]
fn outer_product_add_2x3() {
    assert_truthy("[1 2] @1 [10 20 30] @2 + [[11 21 31][12 22 32]] equals");
}

/// Outer product under multiplication.
#[test]
fn outer_product_multiply() {
    assert_truthy("[10 20] @1 [1 2] @2 * [[10 20][20 40]] equals");
}

/// Outer product: `[1 2 3] @1 [10 20] @2 *` → scale rows.
#[test]
fn outer_product_multiply_3x2() {
    assert_truthy("[1 2 3] @1 [10 20] @2 * [[10 20][20 40][30 60]] equals");
}

// =============================================================================
// 5. REDUCE (`+/`, `-/`, `*/`, `max/`) — finite lists and edge cases
// =============================================================================

/// Sum of [1 2 3 4] via reduce.
#[test]
fn reduce_sum() {
    assert_top_real("[1 2 3 4] +/", 10.0);
}

/// Reduce with `-`: left-associative subtraction.
/// [10 3 2] -/ → (10 - 3) - 2 = 5
#[test]
fn reduce_subtract() {
    assert_top_real("[10 3 2] -/", 5.0);
}

/// Reduce with `*`: factorial-style.
/// [1 2 3 4 5] */ → 120
#[test]
fn reduce_product() {
    assert_top_real("[1 2 3 4 5] */", 120.0);
}

/// Reduce with `max`: maximum of list.
#[test]
fn reduce_max() {
    assert_top_real("[3 1 4 1 5 9 2 6] max/", 9.0);
}

/// Reduce with `min`: minimum of list.
#[test]
fn reduce_min() {
    assert_top_real("[3 1 4 1 5 9 2 6] min/", 1.0);
}

/// Reduce of a single-element list returns that element unchanged.
#[test]
fn reduce_single_element() {
    assert_top_real("[42] +/", 42.0);
}

/// Reduce of a two-element list.
#[test]
fn reduce_two_elements() {
    assert_top_real("[7 3] -/", 4.0);
}

/// Reduce on empty list produces an error (no identity element).
#[test]
fn reduce_empty_list_errors() {
    assert_err("[] +/");
}

// =============================================================================
// 6. SCAN (`+\`, `*\`) — running prefix operations
// =============================================================================

/// Running sum scan on [1 2 3 4].
#[test]
fn scan_sum() {
    assert_truthy("[1 2 3 4] +\\ [1 3 6 10] equals");
}

/// Running product scan on [1 2 3 4 5].
#[test]
fn scan_product() {
    assert_truthy("[1 2 3 4 5] *\\ [1 2 6 24 120] equals");
}

/// Scan of a single element returns a one-element list.
#[test]
fn scan_single_element() {
    assert_truthy("[7] +\\ [7] equals");
}

/// Scan of an empty list returns an empty list.
#[test]
fn scan_empty_list() {
    assert_truthy("[] +\\ [] equals");
}

/// Scan with subtraction: [10 3 2 1] -\ → [10, 10-3=7, 7-2=5, 5-1=4].
#[test]
fn scan_subtract() {
    assert_truthy("[10 3 2 1] -\\ [10 7 5 4] equals");
}

/// Verify individual elements of scan result using `at`.
#[test]
fn scan_element_access() {
    // [1 2 3 4] +\ at index 2 (0-based) should be 6
    assert_top_real("[1 2 3 4] +\\ 2 at", 6.0);
}

// =============================================================================
// 7. PAIRWISE (`+^`, `-^`) — adjacent-pair operations
// =============================================================================

/// Pairwise sum: first element retained, then each adjacent pair summed.
/// [1 2 3 4] +^ → [1, 1+2=3, 2+3=5, 3+4=7]
#[test]
fn pairwise_sum() {
    assert_truthy("[1 2 3 4] +^ [1 3 5 7] equals");
}

/// Pairwise difference: [1 2 3 4] -^ → [1, 2-1=1, 3-2=1, 4-3=1].
#[test]
fn pairwise_diff() {
    assert_truthy("[1 2 3 4] -^ [1 1 1 1] equals");
}

/// Pairwise on a monotonically increasing list gives constant differences.
#[test]
fn pairwise_diff_increasing() {
    assert_truthy("[10 20 30 40 50] -^ [10 10 10 10 10] equals");
}

/// Pairwise of a single element returns that element (no pairs exist).
#[test]
fn pairwise_single() {
    assert_truthy("[5] +^ [5] equals");
}

/// Pairwise of an empty list returns an empty list.
#[test]
fn pairwise_empty() {
    assert_truthy("[] +^ [] equals");
}

/// Pairwise on a two-element list.
#[test]
fn pairwise_two_elements() {
    assert_truthy("[3 7] +^ [3 10] equals");
}

// =============================================================================
// 8. MIXED — compositions across adverbs and each
// =============================================================================

/// Reduce of a scan: [1 2 3 4] +\ → [1 3 6 10]; +/ → 20.
#[test]
fn reduce_of_scan() {
    assert_top_real("[1 2 3 4] +\\ +/", 20.0);
}

/// Map then reduce: [1 2 3] @ sq → [1 4 9]; +/ → 14.
#[test]
fn map_then_reduce() {
    assert_top_real("[1 2 3] @ sq +/", 14.0);
}

/// Scan then take N: [1 2 3 4 5] +\ take first 3 → [1 3 6].
#[test]
fn scan_then_take() {
    assert_truthy("[1 2 3 4 5] +\\ 3 N [1 3 6] equals");
}

/// Map negation then scan: [1 2 3] @ neg → [-1 -2 -3]; +\ → [-1 -3 -6].
#[test]
fn map_neg_then_scan() {
    assert_truthy("[1 2 3] @ neg +\\ [-1 -3 -6] equals");
}

/// Zip two lists, then reduce the result.
/// [1 2 3] @ [10 20 30] @ + → [11 22 33]; +/ → 66.
#[test]
fn zip_then_reduce() {
    assert_top_real("[1 2 3] @ [10 20 30] @ + +/", 66.0);
}

/// Outer product, then map sum of each row.
/// [1 2] @1 [10 20 30] @2 + → [[11 21 31][12 22 32]]; @ +/ → [63 66].
#[test]
fn outer_product_then_row_sum() {
    assert_truthy("[1 2] @1 [10 20 30] @2 + @ \\x [x +/] ! [63 66] equals");
}

// =============================================================================
// 9. INFINITE STREAM ADVERBS — lazy evaluation
// =============================================================================

/// `ord +\` produces a lazy infinite scan; first 5 elements are [1 3 6 10 15].
/// ord = 1,2,3,4,5,...; cumulative sum = 1,3,6,10,15,...
#[test]
fn infinite_scan_ord_sum_first5() {
    assert_truthy("ord +\\ 5 N [1 3 6 10 15] equals");
}

/// `nat +\` produces a lazy infinite scan; first 5 elements are [0 1 3 6 10].
/// nat = 0,1,2,3,4,...; cumulative sum = 0,1,3,6,10,...
#[test]
fn infinite_scan_nat_sum_first5() {
    assert_truthy("nat +\\ 5 N [0 1 3 6 10] equals");
}

/// `ord +\` result is an infinite stream (not materialized).
#[test]
fn infinite_scan_is_infinite() {
    let ops = parse("ord +\\").unwrap();
    let mut i = Interp::new();
    i.exec(&ops).unwrap();
    let top = i.stack.pop().unwrap();
    match &top {
        Value::Stream(s) => assert!(s.is_infinite(), "ord +\\ must be infinite"),
        other => panic!("ord +\\ should be Stream, got: {other:?}"),
    }
}

/// `nat +^` (lazy pairwise add on nat): first 5 = [0, 0+1=1, 1+2=3, 2+3=5, 3+4=7].
#[test]
fn infinite_pairwise_nat_first5() {
    assert_truthy("nat +^ 5 N [0 1 3 5 7] equals");
}

/// `nat +^` result is infinite.
#[test]
fn infinite_pairwise_is_infinite() {
    let ops = parse("nat +^").unwrap();
    let mut i = Interp::new();
    i.exec(&ops).unwrap();
    let top = i.stack.pop().unwrap();
    match &top {
        Value::Stream(s) => assert!(s.is_infinite(), "nat +^ must be infinite"),
        other => panic!("nat +^ should be Stream, got: {other:?}"),
    }
}

/// `nat *\` (lazy product scan): first 5 = [0, 0, 0, 0, 0] since 0 * anything = 0.
/// nat = 0,1,2,3,4; product scan = 0, 0*1=0, 0*2=0, 0*3=0, 0*4=0.
#[test]
fn infinite_scan_nat_product_first5() {
    assert_truthy("nat *\\ 5 N [0 0 0 0 0] equals");
}

/// Reduce on an infinite stream produces an error.
#[test]
fn infinite_reduce_errors() {
    assert_err("nat +/");
}

/// Take N from the lazy scan stream at a higher offset to verify correctness.
/// ord +\ offset 4: skip first 4, take next 3 → elements 5..7 = [15, 21, 28].
/// Triangular numbers: T(5)=15, T(6)=21, T(7)=28.
#[test]
fn infinite_scan_ord_sum_offset() {
    // T(n) = n*(n+1)/2; T(5)=15, T(6)=21, T(7)=28
    assert_truthy("ord +\\ 7 N 4 skip [15 21 28] equals");
}

// =============================================================================
// 10. ERROR CASES
// =============================================================================

/// Reduce of empty list errors.
#[test]
fn error_reduce_empty() {
    assert_err("[] +/");
}

/// Reduce of infinite stream errors.
#[test]
fn error_reduce_infinite() {
    assert_err("nat +/");
}

/// Sorting an infinite stream errors (must materialize first).
#[test]
fn error_sort_infinite() {
    assert_err("nat sort"); // sort must collect the stream → error on infinite
}

/// `nat size` returns 0 (uses len_hint), not an error — the stream is not materialized.
/// This documents the correct behavior: size on infinite stream = 0.
#[test]
fn infinite_size_is_zero() {
    assert_top_real("nat size", 0.0);
}

// =============================================================================
// 11. NESTED RANK — inner reduce/scan via lambdas
// =============================================================================

/// `[[1 2] [3 4]] @ \x [x +/] !` → reduce each inner list → [3 7].
#[test]
fn nested_each_inner_reduce() {
    assert_truthy("[[1 2][3 4]] @ \\x [x +/] ! [3 7] equals");
}

/// Reduce each inner list with `*`: [[1 2 3][4 5 6]] @ \x [x */] ! → [6 120].
#[test]
fn nested_each_inner_product() {
    assert_truthy("[[1 2 3][4 5 6]] @ \\x [x */] ! [6 120] equals");
}

/// Scan each inner list: [[1 2 3][4 5 6]] @ \x [x +\] ! → [[1 3 6][4 9 15]].
#[test]
fn nested_each_inner_scan() {
    assert_truthy("[[1 2 3][4 5 6]] @ \\x [x +\\] ! [[1 3 6][4 9 15]] equals");
}

/// `@@` for 2-deep: [[1 2][3 4]] @@ neg → [[-1 -2][-3 -4]].
#[test]
fn nested_depth2_via_double_at() {
    assert_truthy("[[1 2][3 4]] @@ neg [[-1 -2][-3 -4]] equals");
}

/// Map over a list, then map over the result: [1 2 3] @ sq @ sqrt → [1 2 3].
#[test]
fn map_then_map() {
    assert_truthy("[1 2 3] @ sq @ sqrt [1 2 3] equals");
}

// =============================================================================
// 12. LAMBDAS — map with lambda, captured variables
// =============================================================================

/// Map squaring lambda: [1 2 3] @ \x [x x *] ! → [1 4 9].
#[test]
fn lambda_each_square() {
    assert_truthy("[1 2 3] @ \\x [x x *] ! [1 4 9] equals");
}

/// Map double-and-add-one: [1 2 3] @ \x [x 2 * 1 +] ! → [3 5 7].
#[test]
fn lambda_each_double_plus_one() {
    assert_truthy("[1 2 3] @ \\x [x 2 * 1 +] ! [3 5 7] equals");
}

/// Two-arg lambda in zip mode: [1 2 3] @ [10 20 30] @ \a b [a b + 2 *] ! → [22 44 66].
#[test]
fn lambda_zip_add_double() {
    assert_truthy("[1 2 3] @ [10 20 30] @ \\a b [a b + 2 *] ! [22 44 66] equals");
}

/// Lambda captures a binding from the outer scope.
/// `5 = offset  [1 2 3] @ \x [x offset +] ! → [6 7 8]`
#[test]
fn lambda_captures_binding() {
    assert_truthy("5 = offset  [1 2 3] @ \\x [x offset +] ! [6 7 8] equals");
}

/// Outer product with a lambda.
/// [1 2] @1 [10 20] @2 \a b [a b *] ! → [[10 20][20 40]]
#[test]
fn outer_product_with_lambda() {
    assert_truthy("[1 2] @1 [10 20] @2 \\a b [a b *] ! [[10 20][20 40]] equals");
}
