// Parser stress tests for stax-parser.
// Covers: number literals, string edge cases, word/adverb parsing, comments,
// list nesting, lambdas, forms, bind patterns, adverb suffixes, each words,
// quotes/syms, form access, signal lists, whitespace, and mixed programs.

use stax_core::{op::Adverb, Op, Value};
use stax_parser::parse;

// ---- helper ---------------------------------------------------------------

fn ops(src: &str) -> Vec<Op> {
    parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e:?}"))
}

fn real_val(op: &Op) -> f64 {
    match op {
        Op::Lit(Value::Real(n)) => *n,
        other => panic!("expected Lit(Real), got {other:?}"),
    }
}

fn word_name(op: &Op) -> &str {
    match op {
        Op::Word(s) => s.as_ref(),
        other => panic!("expected Word, got {other:?}"),
    }
}

// ===========================================================================
// 1. Number literals
// ===========================================================================

#[test]
fn num_zero() {
    let o = ops("0");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 0.0).abs() < 1e-12);
}

#[test]
fn num_negative_zero() {
    // -0 is parsed as negative number
    let o = ops("-0");
    assert_eq!(o.len(), 1);
    // -0.0 == 0.0 in IEEE 754
    assert_eq!(real_val(&o[0]), 0.0);
}

#[test]
fn num_suffix_k() {
    let o = ops("1k");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 1_000.0).abs() < 1e-6);
}

#[test]
fn num_suffix_2k() {
    let o = ops("2k");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 2_000.0).abs() < 1e-6);
}

#[test]
fn num_suffix_big_m() {
    // 2M = 2_000_000
    let o = ops("2M");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 2_000_000.0).abs() < 1.0);
}

#[test]
fn num_suffix_small_m() {
    // 1m = 0.001
    let o = ops("1m");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 1e-3).abs() < 1e-12);
}

#[test]
fn num_suffix_u() {
    // 1u = 1e-6
    let o = ops("1u");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 1e-6).abs() < 1e-18);
}

#[test]
fn num_suffix_pi() {
    // 1pi ≈ 3.14159…
    let o = ops("1pi");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - std::f64::consts::PI).abs() < 1e-9);
}

#[test]
fn num_suffix_h() {
    // 1h = 2π (tau)
    let o = ops("1h");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - std::f64::consts::TAU).abs() < 1e-9);
}

#[test]
fn num_fraction_three_quarters() {
    // 3/4 = 0.75
    let o = ops("3/4");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 0.75).abs() < 1e-12);
}

#[test]
fn num_fraction_negative() {
    // -3/4 = -0.75
    let o = ops("-3/4");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - (-0.75)).abs() < 1e-12);
}

#[test]
fn num_scientific_e10() {
    // 1e10
    let o = ops("1e10");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 1e10).abs() < 1.0);
}

#[test]
fn num_scientific_negative_exp() {
    // -2.5e-3
    let o = ops("-2.5e-3");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - (-2.5e-3)).abs() < 1e-15);
}

#[test]
fn num_scientific_upper_e() {
    // 1.23E4
    let o = ops("1.23E4");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 12_300.0).abs() < 1e-6);
}

// ===========================================================================
// 2. String edge cases
// ===========================================================================

#[test]
fn string_empty() {
    let o = ops("\"\"");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Lit(Value::Str(s)) if s.as_ref() == ""));
}

#[test]
fn string_with_newline_escape() {
    let o = ops("\"hello\\nworld\"");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Lit(Value::Str(s)) if s.as_ref() == "hello\nworld"));
}

#[test]
fn string_with_escaped_quote() {
    let o = ops("\"say \\\"hi\\\"\"");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Lit(Value::Str(s)) if s.as_ref() == "say \"hi\""));
}

#[test]
fn string_very_long() {
    let long = "x".repeat(10_000);
    let src = format!("\"{long}\"");
    let o = ops(&src);
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Lit(Value::Str(s)) if s.len() == 10_000));
}

// ===========================================================================
// 3. Word parsing: words starting with operators
// ===========================================================================

#[test]
fn word_plus_prefix() {
    // +myword — should parse as a single Word, not an adverb
    let o = ops("+myword");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "+myword"));
}

#[test]
fn word_at_symbol() {
    // @ stands alone as a word (the "each" word)
    let o = ops("@ ");
    // @ is a valid word in the language; the trailing space should not produce extra ops
    // Behavior: parse_word_or_adverb reads "@", no adverb suffix → Word("@")
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@"));
}

#[test]
fn word_double_at() {
    let o = ops("@@");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@@"));
}

#[test]
fn word_triple_at() {
    let o = ops("@@@");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@@@"));
}

#[test]
fn word_at1() {
    let o = ops("@1");
    assert_eq!(o.len(), 1);
    // @1 has a digit which is not a word char, so it reads "@" then 1 separately
    // is_word_char('1') = true (digits not excluded), so "@1" is one word token
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@1"));
}

#[test]
fn word_at2() {
    let o = ops("@2");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@2"));
}

// ===========================================================================
// 4. Comments
// ===========================================================================

#[test]
fn comment_at_end_of_line() {
    // semicolon comment should be stripped; next line should parse
    let o = ops("; comment at end\n2");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 2.0).abs() < 1e-12);
}

#[test]
fn comment_mid_expression() {
    let o = ops("1 ; ignored comment\n2 +");
    assert_eq!(o.len(), 3);
    assert!((real_val(&o[0]) - 1.0).abs() < 1e-12);
    assert!((real_val(&o[1]) - 2.0).abs() < 1e-12);
    assert!(matches!(&o[2], Op::Word(s) if s.as_ref() == "+"));
}

#[test]
fn comment_only_line() {
    let o = ops("; this is a comment-only line\n; another comment\n");
    assert_eq!(o.len(), 0);
}

// ===========================================================================
// 5. List nesting
// ===========================================================================

#[test]
fn list_deeply_nested_empty() {
    // [[[]]] → ListMark ListMark ListMark MakeList MakeList MakeList
    let o = ops("[[[]]]");
    assert_eq!(o.len(), 6);
    assert!(matches!(o[0], Op::ListMark));
    assert!(matches!(o[1], Op::ListMark));
    assert!(matches!(o[2], Op::ListMark));
    assert!(matches!(o[3], Op::MakeList { signal: false }));
    assert!(matches!(o[4], Op::MakeList { signal: false }));
    assert!(matches!(o[5], Op::MakeList { signal: false }));
}

#[test]
fn list_nested_with_values() {
    // [1 [2 [3]]] → LM Lit(1) LM Lit(2) LM Lit(3) ML(inner) ML(mid) ML(outer)
    // = 9 ops total
    let o = ops("[1 [2 [3]]]");
    assert_eq!(o.len(), 9);
    assert!(matches!(o[0], Op::ListMark));        // outer [
    assert!((real_val(&o[1]) - 1.0).abs() < 1e-12);
    assert!(matches!(o[2], Op::ListMark));        // middle [
    assert!((real_val(&o[3]) - 2.0).abs() < 1e-12);
    assert!(matches!(o[4], Op::ListMark));        // inner [
    assert!((real_val(&o[5]) - 3.0).abs() < 1e-12);
    assert!(matches!(o[6], Op::MakeList { signal: false })); // closes [3]
    assert!(matches!(o[7], Op::MakeList { signal: false })); // closes [2 [3]]
    assert!(matches!(o[8], Op::MakeList { signal: false })); // closes outer
}

// ===========================================================================
// 6. Lambda parsing
// ===========================================================================

#[test]
fn lambda_no_params_empty_body() {
    // \[] — zero params, empty body
    let o = ops("\\[]");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::MakeFun { params, body } if params.is_empty() && body.is_empty()));
}

#[test]
fn lambda_one_param_empty_body() {
    // \a [] — one param, empty body
    let o = ops("\\a []");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::MakeFun { params, body }
        if params.len() == 1 && params[0].as_ref() == "a" && body.is_empty()));
}

#[test]
fn lambda_three_params() {
    // \a b c [a b c + +]
    let o = ops("\\a b c [a b c + +]");
    assert_eq!(o.len(), 1);
    let Op::MakeFun { params, body } = &o[0] else { panic!("expected MakeFun") };
    assert_eq!(params.len(), 3);
    assert_eq!(params[0].as_ref(), "a");
    assert_eq!(params[1].as_ref(), "b");
    assert_eq!(params[2].as_ref(), "c");
    // body: [Word("a"), Word("b"), Word("c"), Word("+"), Word("+")]
    assert_eq!(body.len(), 5);
}

// ===========================================================================
// 7. Form parsing
// ===========================================================================

#[test]
fn form_empty() {
    // {} → MakeForm { keys:[], parent:false }
    let o = ops("{}");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::MakeForm { keys, parent } if keys.is_empty() && !parent));
}

#[test]
fn form_one_key() {
    // {:x 1} → Lit(1), MakeForm { keys:["x"], parent:false }
    let o = ops("{:x 1}");
    assert_eq!(o.len(), 2);
    assert!((real_val(&o[0]) - 1.0).abs() < 1e-12);
    assert!(matches!(&o[1], Op::MakeForm { keys, parent }
        if keys.len() == 1 && keys[0].as_ref() == "x" && !parent));
}

#[test]
fn form_two_keys() {
    // {:x 1 :y 2} → Lit(1), Lit(2), MakeForm { keys:["x","y"], parent:false }
    let o = ops("{:x 1 :y 2}");
    assert_eq!(o.len(), 3);
    assert!((real_val(&o[0]) - 1.0).abs() < 1e-12);
    assert!((real_val(&o[1]) - 2.0).abs() < 1e-12);
    assert!(matches!(&o[2], Op::MakeForm { keys, parent }
        if keys.len() == 2 && keys[0].as_ref() == "x" && keys[1].as_ref() == "y" && !parent));
}

#[test]
fn form_with_parent_expression() {
    // {someForm :x 1} — parent expression precedes the first :key
    // The parent expression pushes someForm onto the stack; has_parent=true
    let o = ops("{someForm :x 1}");
    // someForm → Word("someForm"), then 1 → Lit(1), then MakeForm{parent:true}
    assert_eq!(o.len(), 3);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "someForm"));
    assert!((real_val(&o[1]) - 1.0).abs() < 1e-12);
    assert!(matches!(&o[2], Op::MakeForm { keys, parent }
        if keys.len() == 1 && parent == &true && keys[0].as_ref() == "x"));
}

// ===========================================================================
// 8. Bind patterns
// ===========================================================================

#[test]
fn bind_single_name() {
    let o = ops("= foo");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Bind(n) if n.as_ref() == "foo"));
}

#[test]
fn bind_tuple_two_names() {
    // = (a b) — pops two stack values
    let o = ops("= (a b)");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::BindMany { names, list_mode }
        if names.len() == 2 && names[0].as_ref() == "a" && names[1].as_ref() == "b"
        && !list_mode));
}

#[test]
fn bind_list_destructure() {
    // = [a b c] — destructures a list value
    let o = ops("= [a b c]");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::BindMany { names, list_mode }
        if names.len() == 3 && *list_mode));
}

#[test]
fn bind_tuple_one_name() {
    let o = ops("= (x)");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::BindMany { names, list_mode }
        if names.len() == 1 && names[0].as_ref() == "x" && !list_mode));
}

#[test]
fn bind_tuple_five_names() {
    let o = ops("= (a b c d e)");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::BindMany { names, list_mode }
        if names.len() == 5 && !list_mode));
}

// ===========================================================================
// 9. Adverb suffixes
// ===========================================================================

#[test]
fn adverb_reduce_plus() {
    // +/ → [Adverb(Reduce), Word("+")]
    let o = ops("+/");
    assert_eq!(o.len(), 2);
    assert!(matches!(o[0], Op::Adverb(Adverb::Reduce)));
    assert!(matches!(&o[1], Op::Word(s) if s.as_ref() == "+"));
}

#[test]
fn adverb_scan_plus() {
    // +\ → [Adverb(Scan), Word("+")]
    let o = ops("+\\");
    assert_eq!(o.len(), 2);
    assert!(matches!(o[0], Op::Adverb(Adverb::Scan)));
    assert!(matches!(&o[1], Op::Word(s) if s.as_ref() == "+"));
}

#[test]
fn adverb_pairwise_plus() {
    // +^ → [Adverb(Pairwise), Word("+")]
    let o = ops("+^");
    assert_eq!(o.len(), 2);
    assert!(matches!(o[0], Op::Adverb(Adverb::Pairwise)));
    assert!(matches!(&o[1], Op::Word(s) if s.as_ref() == "+"));
}

#[test]
fn adverb_reduce_star() {
    // */ → [Adverb(Reduce), Word("*")]
    let o = ops("*/");
    assert_eq!(o.len(), 2);
    assert!(matches!(o[0], Op::Adverb(Adverb::Reduce)));
    assert!(matches!(&o[1], Op::Word(s) if s.as_ref() == "*"));
}

#[test]
fn adverb_reduce_minus() {
    // -/ → [Adverb(Reduce), Word("-")]
    let o = ops("-/");
    assert_eq!(o.len(), 2);
    assert!(matches!(o[0], Op::Adverb(Adverb::Reduce)));
    assert!(matches!(&o[1], Op::Word(s) if s.as_ref() == "-"));
}

// ===========================================================================
// 10. Each words
// ===========================================================================

#[test]
fn each_word_at() {
    let o = ops("@");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@"));
}

#[test]
fn each_word_double_at() {
    let o = ops("@@");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@@"));
}

#[test]
fn each_word_triple_at() {
    let o = ops("@@@");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@@@"));
}

#[test]
fn each_word_at1() {
    let o = ops("@1");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@1"));
}

#[test]
fn each_word_at2() {
    let o = ops("@2");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "@2"));
}

// ===========================================================================
// 11. Quote and sym
// ===========================================================================

#[test]
fn quote_word() {
    let o = ops("`word");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Quote(s) if s.as_ref() == "word"));
}

#[test]
fn sym_word() {
    let o = ops("'word");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Sym(s) if s.as_ref() == "word"));
}

#[test]
fn multiple_quote_and_sym() {
    let o = ops("'foo `bar 'baz");
    assert_eq!(o.len(), 3);
    assert!(matches!(&o[0], Op::Sym(s) if s.as_ref() == "foo"));
    assert!(matches!(&o[1], Op::Quote(s) if s.as_ref() == "bar"));
    assert!(matches!(&o[2], Op::Sym(s) if s.as_ref() == "baz"));
}

// ===========================================================================
// 12. Form access
// ===========================================================================

#[test]
fn form_get() {
    let o = ops(",key");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::FormGet(s) if s.as_ref() == "key"));
}

#[test]
fn form_apply() {
    let o = ops(".method");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::FormApply(s) if s.as_ref() == "method"));
}

#[test]
fn multiple_form_access() {
    let o = ops(",x ,y .z");
    assert_eq!(o.len(), 3);
    assert!(matches!(&o[0], Op::FormGet(s) if s.as_ref() == "x"));
    assert!(matches!(&o[1], Op::FormGet(s) if s.as_ref() == "y"));
    assert!(matches!(&o[2], Op::FormApply(s) if s.as_ref() == "z"));
}

// ===========================================================================
// 13. Signal list (#[...])
// ===========================================================================

#[test]
fn signal_list_basic() {
    let o = ops("#[1 2 3]");
    // ListMark, Lit(1), Lit(2), Lit(3), MakeList{signal:true}
    assert_eq!(o.len(), 5);
    assert!(matches!(o[0], Op::ListMark));
    assert!((real_val(&o[1]) - 1.0).abs() < 1e-12);
    assert!((real_val(&o[2]) - 2.0).abs() < 1e-12);
    assert!((real_val(&o[3]) - 3.0).abs() < 1e-12);
    assert!(matches!(o[4], Op::MakeList { signal: true }));
}

#[test]
fn signal_list_empty() {
    let o = ops("#[]");
    assert_eq!(o.len(), 2);
    assert!(matches!(o[0], Op::ListMark));
    assert!(matches!(o[1], Op::MakeList { signal: true }));
}

// ===========================================================================
// 14. Whitespace handling
// ===========================================================================

#[test]
fn whitespace_tabs() {
    let o = ops("1\t2\t+");
    assert_eq!(o.len(), 3);
    assert!((real_val(&o[0]) - 1.0).abs() < 1e-12);
    assert!((real_val(&o[1]) - 2.0).abs() < 1e-12);
    assert!(matches!(&o[2], Op::Word(s) if s.as_ref() == "+"));
}

#[test]
fn whitespace_multiple_spaces() {
    let o = ops("1   2   +");
    assert_eq!(o.len(), 3);
}

#[test]
fn whitespace_newlines_between_tokens() {
    let o = ops("1\n2\n+");
    assert_eq!(o.len(), 3);
}

#[test]
fn whitespace_leading_and_trailing() {
    let o = ops("   42   ");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 42.0).abs() < 1e-12);
}

// ===========================================================================
// 15. Mixed complex program
// ===========================================================================

#[test]
fn complex_program_stats_lambda() {
    // \a b [a b + a b * 2ple] = stats
    // then: 3 4 `stats !
    let o = ops("\\a b [a b + a b * 2ple] = stats  3 4 `stats !");
    // Op breakdown:
    //   0: MakeFun { params:[a,b], body:[Word(a),Word(b),Word(+),Word(a),Word(b),Word(*),Word(2ple)] }
    //   1: Bind("stats")
    //   2: Lit(3)
    //   3: Lit(4)
    //   4: Quote("stats")
    //   5: Call
    assert_eq!(o.len(), 6);
    assert!(matches!(&o[0], Op::MakeFun { params, .. } if params.len() == 2));
    assert!(matches!(&o[1], Op::Bind(n) if n.as_ref() == "stats"));
    assert!((real_val(&o[2]) - 3.0).abs() < 1e-12);
    assert!((real_val(&o[3]) - 4.0).abs() < 1e-12);
    assert!(matches!(&o[4], Op::Quote(s) if s.as_ref() == "stats"));
    assert!(matches!(o[5], Op::Call));
}

// ===========================================================================
// 16. Additional edge cases
// ===========================================================================

#[test]
fn num_suffix_pi_scaled() {
    // 2pi = 2 * π
    let o = ops("2pi");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 2.0 * std::f64::consts::PI).abs() < 1e-9);
}

#[test]
fn num_suffix_h_scaled() {
    // 2h = 2 * τ
    let o = ops("2h");
    assert_eq!(o.len(), 1);
    assert!((real_val(&o[0]) - 2.0 * std::f64::consts::TAU).abs() < 1e-9);
}

#[test]
fn multiple_numbers_in_sequence() {
    let o = ops("1 2 3 4 5");
    assert_eq!(o.len(), 5);
    for (i, op) in o.iter().enumerate() {
        assert!((real_val(op) - (i + 1) as f64).abs() < 1e-12);
    }
}

#[test]
fn call_operator() {
    let o = ops("!");
    assert_eq!(o.len(), 1);
    assert!(matches!(o[0], Op::Call));
}

#[test]
fn lambda_in_list() {
    // A lambda nested inside a list literal
    let o = ops("[\\x [x x *]]");
    // ListMark, MakeFun{params:[x], body:[Word(x),Word(x),Word(*)]}, MakeList
    assert_eq!(o.len(), 3);
    assert!(matches!(o[0], Op::ListMark));
    assert!(matches!(&o[1], Op::MakeFun { params, body }
        if params.len() == 1 && params[0].as_ref() == "x" && body.len() == 3));
    assert!(matches!(o[2], Op::MakeList { signal: false }));
}

#[test]
fn adverb_sequences() {
    // +/ +\ +^ in one expression
    let o = ops("+/ +\\ +^");
    assert_eq!(o.len(), 6);
    assert!(matches!(o[0], Op::Adverb(Adverb::Reduce)));
    assert_eq!(word_name(&o[1]), "+");
    assert!(matches!(o[2], Op::Adverb(Adverb::Scan)));
    assert_eq!(word_name(&o[3]), "+");
    assert!(matches!(o[4], Op::Adverb(Adverb::Pairwise)));
    assert_eq!(word_name(&o[5]), "+");
}

#[test]
fn string_with_tab_escape() {
    let o = ops("\"col1\\tcol2\"");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Lit(Value::Str(s)) if s.as_ref() == "col1\tcol2"));
}

#[test]
fn word_with_underscore() {
    let o = ops("my_word");
    assert_eq!(o.len(), 1);
    assert!(matches!(&o[0], Op::Word(s) if s.as_ref() == "my_word"));
}
