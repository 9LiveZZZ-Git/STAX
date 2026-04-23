use std::sync::Arc;

/// Audio-rate signal. A *description* of something that produces samples.
///
/// Like `Stream`, a `Signal` is reusable — calling `instantiate` gives you
/// a fresh `SignalInstance` with its own phase, state, etc. This is what
/// lets `play` be called twice on the same signal value and get two
/// independent voices.
pub trait Signal: Send + Sync {
    /// Build a running instance for playback at the given sample rate.
    fn instantiate(&self, sample_rate: f64) -> Box<dyn SignalInstance>;

    /// Number of output channels. Multi-channel expansion produces these
    /// by wrapping N single-channel signals.
    fn channels(&self) -> usize {
        1
    }

    /// Known length in samples, for array-backed signals.
    fn len_hint(&self) -> Option<usize> {
        None
    }

    /// For array-backed signals (`#[...]`), returns the underlying f32 samples.
    fn as_f32_slice(&self) -> Option<&[f32]> {
        None
    }

    /// Pull n samples. Array-backed signals slice; lazy signals iterate.
    fn take_n(&self, n: usize) -> Vec<f32> {
        self.as_f32_slice()
            .map(|s| s.iter().take(n).copied().collect())
            .unwrap_or_else(|| vec![0.0f32; n])
    }
}

// -------- lazy generator signal -------------------------------------------

/// Infinite signal backed by a repeatable iterator factory.
/// Used for `ordz` and similar primitives that are signal-typed but infinite.
pub struct GenSignal<F>
where
    F: Fn() -> Box<dyn Iterator<Item = f32> + Send> + Send + Sync + 'static,
{
    factory: F,
}

impl<F> GenSignal<F>
where
    F: Fn() -> Box<dyn Iterator<Item = f32> + Send> + Send + Sync + 'static,
{
    pub fn new(factory: F) -> Self {
        Self { factory }
    }
}

impl<F> Signal for GenSignal<F>
where
    F: Fn() -> Box<dyn Iterator<Item = f32> + Send> + Send + Sync + 'static,
{
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(SilenceInstance)
    }

    fn take_n(&self, n: usize) -> Vec<f32> {
        (self.factory)().take(n).collect()
    }
}

/// Running instance of a signal. Owned by a single audio callback.
///
/// The evaluator will pre-allocate a block size (typically 64 samples) and
/// call `fill` once per block per instance. Implementors should not allocate
/// on the audio thread.
pub trait SignalInstance: Send {
    /// Write exactly `out.len() / channels` frames of interleaved output.
    /// `out.len()` will always be a multiple of `self.channels()`.
    fn fill(&mut self, out: &mut [f32]);

    fn channels(&self) -> usize {
        1
    }
}

// -------- silence --------------------------------------------------------

/// Produces a block of silence. Used by `GenSignal::instantiate` as a no-op
/// placeholder instance (GenSignal is pull-only via `take_n`; the instance
/// is never actually driven during normal use).
pub(crate) struct SilenceInstance;

impl SignalInstance for SilenceInstance {
    fn fill(&mut self, out: &mut [f32]) {
        out.fill(0.0);
    }
}

// -------- signal combinators for lazy composition -------------------------

/// Binary combination of two signals: out[i] = op(a[i], b[i]).
/// Created by `automap_bin` when neither signal has an in-memory slice.
pub struct BinarySignal {
    pub a: Arc<dyn Signal>,
    pub b: Arc<dyn Signal>,
    pub op: fn(f64, f64) -> f64,
}

impl Signal for BinarySignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(BinaryInstance {
            a: self.a.instantiate(sr),
            b: self.b.instantiate(sr),
            op: self.op,
            scratch: Vec::new(),
        })
    }
}

struct BinaryInstance {
    a: Box<dyn SignalInstance>,
    b: Box<dyn SignalInstance>,
    op: fn(f64, f64) -> f64,
    scratch: Vec<f32>,
}

impl SignalInstance for BinaryInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.scratch.resize(out.len(), 0.0);
        self.a.fill(out);
        self.b.fill(&mut self.scratch);
        let op = self.op;
        for (o, &s) in out.iter_mut().zip(self.scratch.iter()) {
            *o = op(*o as f64, s as f64) as f32;
        }
    }
}

/// Unary transformation of every sample: out[i] = op(in[i]).
pub struct UnarySignal {
    pub inner: Arc<dyn Signal>,
    pub op: fn(f64) -> f64,
}

impl Signal for UnarySignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(UnaryInstance {
            inner: self.inner.instantiate(sr),
            op: self.op,
        })
    }
}

struct UnaryInstance {
    inner: Box<dyn SignalInstance>,
    op: fn(f64) -> f64,
}

impl SignalInstance for UnaryInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.inner.fill(out);
        let op = self.op;
        for s in out.iter_mut() {
            *s = op(*s as f64) as f32;
        }
    }
}

/// DC signal — every sample is the same constant value.
pub struct ConstSignal {
    pub value: f32,
}

impl Signal for ConstSignal {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(ConstInstance { value: self.value })
    }
}

struct ConstInstance {
    value: f32,
}

impl SignalInstance for ConstInstance {
    fn fill(&mut self, out: &mut [f32]) {
        out.fill(self.value);
    }
}
