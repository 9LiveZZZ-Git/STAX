//! Core types for stax.
//!
//! This crate defines the runtime `Value` representation, the `Stream` and
//! `Signal` traits (the two flavors of lazy sequence SAPF uses), the `Form`
//! and `Function` types, and the compiled `Op` stream that both the text
//! parser and the graph editor emit into.
//!
//! Nothing in this crate evaluates anything — the interpreter lives in
//! `stax-eval`.

pub mod error;
pub mod form;
pub mod function;
pub mod op;
pub mod signal;
pub mod stream;
pub mod value;

pub use error::{Error, Result};
pub use form::Form;
pub use function::{Function, FunctionBody};
pub use op::{Adverb, Op};
pub use signal::{Signal, SignalInstance};
pub use stream::{Stream, StreamIter};
pub use value::{Value, ValueKind};
