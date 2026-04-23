use std::fmt;
use std::sync::{Arc, RwLock};

use crate::form::Form;
use crate::function::Function;
use crate::signal::Signal;
use crate::stream::Stream;

/// The canonical runtime value.
///
/// All variants except `Ref` are shared-immutable. `Clone` is cheap everywhere
/// because the payloads are either `Copy` or inside `Arc`. This is the whole
/// reason Rust is a good host for SAPF — `Arc<T>` replaces the C++ refcount
/// idiom one-for-one, and `Send + Sync` falls out of immutability.
#[derive(Clone)]
pub enum Value {
    /// 64-bit float. SAPF's only numeric scalar type.
    Real(f64),

    /// Immutable UTF-8 string.
    Str(Arc<str>),

    /// Quoted symbol, e.g. `'sin`. Distinct from `Str` — symbols look up bindings.
    Sym(Arc<str>),

    /// Lazy value sequence. Produces a fresh `StreamIter` on each pull; a
    /// `Stream` is a *description*, not a running iterator. Reusable.
    Stream(Arc<dyn Stream>),

    /// Audio-rate buffer producer. Produces a fresh `SignalInstance` per pull.
    /// Multi-channel expansion is handled at the evaluator level, not here.
    Signal(Arc<dyn Signal>),

    /// Dictionary with inheritance. See `form.rs`.
    Form(Arc<Form>),

    /// First-class function with captured environment.
    Fun(Arc<Function>),

    /// The only mutable value. Used sparingly — SAPF is nearly pure.
    Ref(Arc<RwLock<Value>>),

    /// Unit / absence. Returned by sink words like `play`.
    Nil,
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Real(_) => ValueKind::Real,
            Value::Str(_) => ValueKind::Str,
            Value::Sym(_) => ValueKind::Sym,
            Value::Stream(_) => ValueKind::Stream,
            Value::Signal(_) => ValueKind::Signal,
            Value::Form(_) => ValueKind::Form,
            Value::Fun(_) => ValueKind::Fun,
            Value::Ref(_) => ValueKind::Ref,
            Value::Nil => ValueKind::Nil,
        }
    }

    pub fn as_real(&self) -> Option<f64> {
        if let Value::Real(x) = self {
            Some(*x)
        } else {
            None
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Real(x) => *x != 0.0,
            Value::Str(s) => !s.is_empty(),
            _ => true,
        }
    }
}

/// Tag for error messages and port types in the editor.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ValueKind {
    Real,
    Str,
    Sym,
    Stream,
    Signal,
    Form,
    Fun,
    Ref,
    Nil,
}

impl ValueKind {
    pub fn name(self) -> &'static str {
        match self {
            ValueKind::Real => "Real",
            ValueKind::Str => "Str",
            ValueKind::Sym => "Sym",
            ValueKind::Stream => "Stream",
            ValueKind::Signal => "Signal",
            ValueKind::Form => "Form",
            ValueKind::Fun => "Fun",
            ValueKind::Ref => "Ref",
            ValueKind::Nil => "Nil",
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Real(x) => write!(f, "{x}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Sym(s) => write!(f, "'{s}"),
            Value::Stream(_) => write!(f, "<stream>"),
            Value::Signal(_) => write!(f, "<signal>"),
            Value::Form(_) => write!(f, "<form>"),
            Value::Fun(_) => write!(f, "<fun>"),
            Value::Ref(_) => write!(f, "<ref>"),
            Value::Nil => write!(f, "nil"),
        }
    }
}
