use crate::value::Value;

/// A lazy, possibly infinite, reusable sequence of `Value`s.
///
/// Implementors are *descriptions*, not running iterators. Calling `iter`
/// produces a fresh `StreamIter`. This lets the same `Stream` value be
/// realized multiple times — crucial because SAPF values are immutable and
/// shared.
///
/// Memoization (iterate-once, cache-forever) is deliberately not part of
/// this trait. Some streams are cheap to recompute (`ord`, `nat`), others
/// are not (user-defined generators). A `MemoStream` wrapper can be added
/// as an optimization when profiling justifies it.
pub trait Stream: Send + Sync {
    /// Fresh iterator from the head of this stream.
    fn iter(&self) -> Box<dyn StreamIter>;

    /// Upper bound on length, if known finite. `None` means unknown or infinite.
    fn len_hint(&self) -> Option<usize> {
        None
    }

    /// Is this stream known to be infinite? Used to reject operations like
    /// `reverse` that would never terminate.
    fn is_infinite(&self) -> bool {
        false
    }
}

/// One pull per `next()`. `Send` but not `Sync` — each realization is single-threaded.
pub trait StreamIter: Send {
    fn next(&mut self) -> Option<Value>;
}

/// Any `Iterator<Item=Value> + Send` is a valid `StreamIter`.
impl<I: Iterator<Item = Value> + Send> StreamIter for I {
    fn next(&mut self) -> Option<Value> {
        Iterator::next(self)
    }
}

// -------- common concrete streams ----------------------------------------

/// Wrap a Rust iterator factory as a `Stream`.
///
/// ```ignore
/// let s = IterStream::new(|| Box::new((0..10).map(|i| Value::Real(i as f64))));
/// ```
pub struct IterStream<F>
where
    F: Fn() -> Box<dyn StreamIter> + Send + Sync + 'static,
{
    factory: F,
    len: Option<usize>,
    infinite: bool,
}

impl<F> IterStream<F>
where
    F: Fn() -> Box<dyn StreamIter> + Send + Sync + 'static,
{
    pub fn new(factory: F) -> Self {
        Self {
            factory,
            len: None,
            infinite: false,
        }
    }

    pub fn finite(factory: F, len: usize) -> Self {
        Self {
            factory,
            len: Some(len),
            infinite: false,
        }
    }

    pub fn infinite(factory: F) -> Self {
        Self {
            factory,
            len: None,
            infinite: true,
        }
    }
}

impl<F> Stream for IterStream<F>
where
    F: Fn() -> Box<dyn StreamIter> + Send + Sync + 'static,
{
    fn iter(&self) -> Box<dyn StreamIter> {
        (self.factory)()
    }

    fn len_hint(&self) -> Option<usize> {
        self.len
    }

    fn is_infinite(&self) -> bool {
        self.infinite
    }
}

/// Eager vector-backed stream. Good for literals `[1 2 3]`.
pub struct VecStream(pub Vec<Value>);

impl Stream for VecStream {
    fn iter(&self) -> Box<dyn StreamIter> {
        Box::new(VecStreamIter {
            items: self.0.clone(),
            pos: 0,
        })
    }

    fn len_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

pub struct VecStreamIter {
    items: Vec<Value>,
    pos: usize,
}

impl StreamIter for VecStreamIter {
    fn next(&mut self) -> Option<Value> {
        let v = self.items.get(self.pos).cloned();
        if v.is_some() {
            self.pos += 1;
        }
        v
    }
}
