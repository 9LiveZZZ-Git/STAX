use std::collections::BTreeMap;
use std::sync::Arc;

use crate::value::Value;

/// A SAPF Form: a dictionary with single or multiple inheritance.
///
/// Lookup walks `self.bindings` first, then traverses parents in the order
/// given. When multiple parents are present, SAPF uses the Dylan C3
/// linearization; for v1 we do a simple depth-first left-to-right walk and
/// note this as a follow-up.
#[derive(Clone)]
pub struct Form {
    pub bindings: BTreeMap<Arc<str>, Value>,
    pub parents: Vec<Arc<Form>>,
}

impl Form {
    pub fn new() -> Self {
        Self { bindings: BTreeMap::new(), parents: Vec::new() }
    }

    pub fn with_parent(parent: Arc<Form>) -> Self {
        Self { bindings: BTreeMap::new(), parents: vec![parent] }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        if let Some(v) = self.bindings.get(key) {
            return Some(v.clone());
        }
        for p in &self.parents {
            if let Some(v) = p.get(key) {
                return Some(v);
            }
        }
        None
    }

    pub fn insert(&mut self, key: Arc<str>, value: Value) {
        self.bindings.insert(key, value);
    }
}

impl Default for Form {
    fn default() -> Self {
        Self::new()
    }
}
