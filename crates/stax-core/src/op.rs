use std::sync::Arc;

use crate::value::Value;

/// Compiled operation. Both `stax-parser` and `stax-graph` emit `Vec<Op>`;
/// `stax-eval` is the only consumer. This is the narrow waist that makes
/// text ↔ graph round-tripping tractable.
///
/// Kept deliberately flat. No nested blocks at the `Op` level — lists and
/// function bodies are compiled into `MakeList` / `MakeFun` with inline
/// captured bodies.
#[derive(Clone, Debug)]
pub enum Op {
    /// Push a literal value.
    Lit(Value),

    /// Look up the binding for this word and apply it if it's a function;
    /// otherwise push the value. The default evaluation mode.
    Word(Arc<str>),

    /// `` `word `` — push the binding without applying it (even if it's a function).
    Quote(Arc<str>),

    /// `'word` — push the symbol itself.
    Sym(Arc<str>),

    /// `,word` — pop a Form, look up `word` in it, push the value.
    FormGet(Arc<str>),

    /// `.word` — pop a Form, look up `word`, apply it.
    FormApply(Arc<str>),

    /// `= word` — bind top-of-stack to `word` in the current scope.
    Bind(Arc<str>),

    /// `= (a b c)` or `= [a b c]` — multi-bind. `list_mode` true for `[...]`
    /// destructuring a list, false for `(...)` popping N stack values.
    BindMany { names: Arc<[Arc<str>]>, list_mode: bool },

    /// `!` — apply the function on top of the stack.
    Call,

    /// Build a list from the top `count` stack values.
    /// For value-list literals `[...]`, `count` is dynamic and determined by
    /// how many values the bracketed expression leaves on the stack; the
    /// compiler emits a `ListMark` + `MakeList` pair around the body.
    ListMark,
    MakeList { signal: bool },

    /// Build a Form. Keys in `keys`, values taken from stack.
    MakeForm { keys: Arc<[Arc<str>]>, parent: bool },

    /// Build a Function closing over the current environment.
    MakeFun { params: Arc<[Arc<str>]>, body: Arc<[Op]> },

    /// Rank-lift annotation on the next value consumption.
    /// `depth` 1 for `@`, 2 for `@@`, etc. `order` is the `@1`/`@2` tag for
    /// outer products (0 means unordered).
    Each { depth: u8, order: u8 },

    /// Adverb applied to the next binary operator.
    /// - `Reduce` — `+/` folds across a list
    /// - `Scan`   — `+\` produces running accumulation
    /// - `Pairwise` — `+^` applies between adjacent elements
    Adverb(Adverb),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Adverb {
    Reduce,
    Scan,
    Pairwise,
}
