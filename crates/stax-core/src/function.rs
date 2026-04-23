use std::sync::Arc;

use crate::op::Op;
use crate::value::Value;

/// First-class function.
///
/// SAPF functions are written `\a b [body]`. The body is a sequence of `Op`s
/// that executes on an *empty* stack — arguments are only accessible via
/// their names. This is unlike classic concatenative languages and is why
/// we carry captured bindings explicitly.
///
/// There are two flavors: user-defined (`UserFn`) and built-in (`NativeFn`).
/// Both live here so a function port in the graph editor doesn't need to
/// care which kind it's holding.
pub struct Function {
    pub params: Vec<Arc<str>>,
    pub help: Option<Arc<str>>,
    pub body: FunctionBody,
    pub captured: Vec<(Arc<str>, Value)>,
}

pub type NativeFnPtr = Arc<dyn Fn(&[Value]) -> crate::Result<Vec<Value>> + Send + Sync>;

pub enum FunctionBody {
    /// User-written SAPF code, compiled to an Op stream.
    User(Arc<[Op]>),
    /// Rust closure. Takes named args in order, returns pushed values.
    Native(NativeFnPtr),
}

impl Function {
    pub fn user(params: Vec<Arc<str>>, body: Arc<[Op]>) -> Self {
        Self { params, help: None, body: FunctionBody::User(body), captured: Vec::new() }
    }

    pub fn native<F>(params: Vec<Arc<str>>, f: F) -> Self
    where
        F: Fn(&[Value]) -> crate::Result<Vec<Value>> + Send + Sync + 'static,
    {
        Self {
            params,
            help: None,
            body: FunctionBody::Native(Arc::new(f)),
            captured: Vec::new(),
        }
    }

    pub fn arity(&self) -> usize {
        self.params.len()
    }
}
