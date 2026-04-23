use std::collections::HashMap;
use std::sync::Arc;

use stax_core::Value;

/// Lexical environment. A chain of frames; lookup walks outward.
///
/// SAPF forbids rebinding within a scope (except the top-level workspace).
/// The `bind` method returns `false` if the name already exists in the
/// current frame and we're not at the workspace.
#[derive(Default)]
pub struct Env {
    frames: Vec<Frame>,
}

struct Frame {
    bindings: HashMap<Arc<str>, Value>,
    is_workspace: bool,
}

impl Env {
    pub fn new() -> Self {
        Self {
            frames: vec![Frame {
                bindings: HashMap::new(),
                is_workspace: true,
            }],
        }
    }

    /// Enter a new lexical scope (e.g. a `[...]` block or function body).
    pub fn push_scope(&mut self) {
        self.frames.push(Frame {
            bindings: HashMap::new(),
            is_workspace: false,
        });
    }

    pub fn pop_scope(&mut self) {
        // Never pop the workspace frame.
        if self.frames.len() > 1 {
            self.frames.pop();
        }
    }

    pub fn lookup(&self, name: &str) -> Option<Value> {
        for frame in self.frames.iter().rev() {
            if let Some(v) = frame.bindings.get(name) {
                return Some(v.clone());
            }
        }
        None
    }

    /// Bind `name` to `value` in the current frame.
    /// Returns `false` if the name is already bound in a non-workspace frame.
    pub fn bind(&mut self, name: Arc<str>, value: Value) -> bool {
        let frame = self.frames.last_mut().expect("env frames nonempty");
        if !frame.is_workspace && frame.bindings.contains_key(&name) {
            return false;
        }
        frame.bindings.insert(name, value);
        true
    }

    /// Capture the current environment as a flat `Vec` for closures.
    /// Outer frames shadow inner only where names collide.
    pub fn capture(&self) -> Vec<(Arc<str>, Value)> {
        let mut out: HashMap<Arc<str>, Value> = HashMap::new();
        for frame in &self.frames {
            for (k, v) in &frame.bindings {
                out.insert(k.clone(), v.clone());
            }
        }
        out.into_iter().collect()
    }
}
