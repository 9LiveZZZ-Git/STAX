//! The SAPF stack-machine interpreter.
//!
//! An `Interp` holds the value stack and the lexical environment. Feed it a
//! `&[Op]` from the parser or graph, get back side effects (bindings, pushed
//! values, audio started) and a possibly-modified stack.

pub mod env;
pub mod interp;

pub use env::Env;
pub use interp::query_audio_stat;
pub use interp::Interp;
