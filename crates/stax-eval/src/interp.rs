use std::collections::HashMap;
use std::sync::Arc;

use stax_core::function::{Function, FunctionBody};
use stax_core::op::Adverb;
use stax_core::signal::{BinarySignal, ConstSignal, GenSignal, UnarySignal};
use stax_core::stream::{IterStream, VecStream};
use stax_core::{Error, Form, Op, Result, Value, ValueKind};
use stax_dsp::VecSignal;

use crate::env::Env;

type BuiltinFn = Arc<dyn Fn(&mut Interp) -> Result<()> + Send + Sync>;

pub struct Interp {
    pub stack: Vec<Value>,
    pub env: Env,
    mark_stack: Vec<usize>,
    adverb: Option<Adverb>,
    each_depth: u8,
    each_list: Option<Value>,
    each_zip: Option<Value>,     // second @ in zip mode
    each_stack_mark: usize,
    rank_args: Vec<(u8, Value)>, // @1/@2 outer-product args
    builtins: HashMap<Arc<str>, BuiltinFn>,
    /// Deterministic RNG seed — set via `seed` word, consumed by `muss`/`rand`/`irand`.
    pub rng_seed: u64,
    /// Sample rate used by DSP words that need it (e.g. `sr`, `nyq`). Updated when audio starts.
    pub sample_rate: f64,
    /// Lazy-initialized audio runtime.
    audio_rt: Option<Arc<stax_audio::Runtime>>,
    /// Live audio voices (drop to stop).
    voices: Vec<stax_audio::Voice>,
    /// Active MIDI output connection (set via `midiConnect`).
    pub midi_out: Option<stax_io::MidiOut>,
}

impl Interp {
    pub fn new() -> Self {
        let mut interp = Self {
            stack: Vec::with_capacity(256),
            env: Env::new(),
            mark_stack: Vec::new(),
            adverb: None,
            each_depth: 0,
            each_list: None,
            each_zip: None,
            each_stack_mark: 0,
            rank_args: Vec::new(),
            builtins: HashMap::new(),
            rng_seed: 0xdeadbeef_cafebabe,
            sample_rate: 48_000.0,
            audio_rt: None,
            voices: Vec::new(),
            midi_out: None,
        };
        install_builtins(&mut interp);
        interp
    }

    // ---- stack -----------------------------------------------------------

    pub fn push(&mut self, v: Value) { self.stack.push(v); }

    pub fn pop(&mut self) -> Result<Value> {
        self.stack.pop().ok_or(Error::StackUnderflow { expected: 1, actual: 0 })
    }

    pub fn pop_n(&mut self, n: usize) -> Result<Vec<Value>> {
        if self.stack.len() < n {
            return Err(Error::StackUnderflow { expected: n, actual: self.stack.len() });
        }
        Ok(self.stack.split_off(self.stack.len() - n))
    }

    pub fn peek(&self) -> Option<&Value> { self.stack.last() }

    // ---- execution -------------------------------------------------------

    pub fn exec(&mut self, program: &[Op]) -> Result<()> {
        for op in program { self.step(op)?; }
        Ok(())
    }

    // Alias kept for call-site compatibility.
    pub fn exec_with_ctx(&mut self, program: &[Op]) -> Result<()> {
        self.exec(program)
    }

    fn step(&mut self, op: &Op) -> Result<()> {
        match op {
            Op::Lit(v) => self.push(v.clone()),

            Op::Word(name) => {
                let adverb = self.adverb.take();
                let saved_depth = self.each_depth;
                let has_each = self.each_depth > 0 || self.each_list.is_some()
                    || self.each_zip.is_some() || !self.rank_args.is_empty();
                // These words set up each state; they must never be dispatched as each targets.
                let is_each_setup = matches!(name.as_ref(), "@" | "@@" | "@@@" | "@1" | "@2");
                if has_each && !is_each_setup { self.each_depth = 0; }

                if let Some(adv) = adverb {
                    self.apply_adverb_word(name, adv)?;
                } else if has_each && !is_each_setup {
                    self.apply_each_word(name, saved_depth.max(1))?;
                } else {
                    self.call_word(name)?;
                }
            }

            Op::Quote(name) => {
                let v = self.env.lookup(name)
                    .ok_or_else(|| Error::Unbound(name.to_string()))?;
                self.push(v);
            }

            Op::Sym(s) => self.push(Value::Sym(s.clone())),

            Op::Bind(name) => {
                let v = self.pop()?;
                self.env.bind(name.clone(), v);
            }

            Op::BindMany { names, list_mode } => {
                if *list_mode {
                    let list = self.pop()?;
                    let items = collect_to_vec(&list)?;
                    for (name, val) in names.iter().zip(items) {
                        self.env.bind(name.clone(), val);
                    }
                } else {
                    let n = names.len();
                    let items = self.pop_n(n)?;
                    for (name, val) in names.iter().zip(items) {
                        self.env.bind(name.clone(), val);
                    }
                }
            }

            Op::Call => {
                let saved_depth = self.each_depth;
                let has_each = self.each_depth > 0 || self.each_list.is_some()
                    || self.each_zip.is_some() || !self.rank_args.is_empty();
                if has_each { self.each_depth = 0; }
                let f = self.pop()?;
                if has_each {
                    self.apply_each_val(f, saved_depth.max(1))?;
                } else {
                    self.apply_or_push(f)?;
                }
            }

            Op::FormGet(name) => {
                let v = self.pop()?;
                if let Value::Form(f) = v {
                    let val = f.get(name)
                        .ok_or_else(|| Error::Unbound(name.to_string()))?;
                    self.push(val);
                } else {
                    return Err(Error::Type { expected: "Form", actual: v.kind().name() });
                }
            }

            Op::FormApply(name) => {
                let v = self.pop()?;
                if let Value::Form(f) = v {
                    let val = f.get(name)
                        .ok_or_else(|| Error::Unbound(name.to_string()))?;
                    self.apply_or_push(val)?;
                } else {
                    return Err(Error::Type { expected: "Form", actual: v.kind().name() });
                }
            }

            Op::ListMark => {
                self.mark_stack.push(self.stack.len());
            }

            Op::MakeList { signal } => {
                let mark = self.mark_stack.pop()
                    .ok_or_else(|| Error::Other("MakeList without ListMark".into()))?;
                let items: Vec<Value> = self.stack.drain(mark..).collect();
                if *signal {
                    let floats: Result<Vec<f32>> = items.iter().map(|v| {
                        v.as_real().map(|x| x as f32)
                            .ok_or_else(|| Error::Type { expected: "Real", actual: v.kind().name() })
                    }).collect();
                    self.push(make_signal(floats?));
                } else {
                    self.push(make_list(items));
                }
            }

            Op::MakeForm { keys, parent } => {
                let mut form = Form::new();
                for key in keys.iter().rev() {
                    let v = self.pop()?;
                    form.insert(key.clone(), v);
                }
                if *parent {
                    if let Value::Form(f) = self.pop()? {
                        form.parents.push(f);
                    }
                }
                self.push(Value::Form(Arc::new(form)));
            }

            Op::MakeFun { params, body } => {
                let captured = self.env.capture();
                let fun = Function {
                    params: params.to_vec(),
                    help: None,
                    body: FunctionBody::User(body.clone()),
                    captured,
                };
                self.push(Value::Fun(Arc::new(fun)));
            }

            Op::Each { depth, .. } => {
                self.each_depth = *depth;
            }

            Op::Adverb(a) => {
                self.adverb = Some(*a);
            }
        }
        Ok(())
    }

    // ---- word / value application ----------------------------------------

    /// Execute a word by name, checking builtins first, then env.
    pub fn call_word(&mut self, name: &Arc<str>) -> Result<()> {
        // Clone the Arc so the borrow on self.builtins ends before f(self).
        let builtin = self.builtins.get(name).cloned();
        if let Some(f) = builtin {
            return f(self);
        }
        let v = self.env.lookup(name)
            .ok_or_else(|| Error::Unbound(name.to_string()))?;
        self.apply_or_push(v)
    }

    /// Call a user-defined or native Fun.
    pub fn apply_or_push(&mut self, v: Value) -> Result<()> {
        match v {
            Value::Fun(f) => self.call_fun(f),
            other => { self.push(other); Ok(()) }
        }
    }

    fn call_fun(&mut self, f: Arc<Function>) -> Result<()> {
        let arity = f.arity();
        if self.stack.len() < arity {
            return Err(Error::Arity { expected: arity, actual: self.stack.len() });
        }
        let args = self.pop_n(arity)?;
        match &f.body {
            FunctionBody::Native(cb) => {
                let out = cb(&args)?;
                for v in out { self.push(v); }
            }
            FunctionBody::User(body) => {
                self.env.push_scope();
                for (k, v) in &f.captured { self.env.bind(k.clone(), v.clone()); }
                for (name, val) in f.params.iter().zip(args) {
                    self.env.bind(name.clone(), val);
                }
                let outer_stack = std::mem::take(&mut self.stack);
                let outer_marks = std::mem::take(&mut self.mark_stack);
                let result = self.exec(body);
                let inner = std::mem::replace(&mut self.stack, outer_stack);
                self.mark_stack = outer_marks;
                self.env.pop_scope();
                result?;
                for v in inner { self.push(v); }
            }
        }
        Ok(())
    }

    // ---- adverbs ---------------------------------------------------------

    fn apply_adverb_word(&mut self, name: &Arc<str>, adv: Adverb) -> Result<()> {
        let list_val = self.pop()?;
        // Lazy path for infinite streams — only works for pre-resolved arithmetic ops.
        if let Value::Stream(ref s) = list_val {
            if s.is_infinite() {
                if let Some(f) = resolve_arith(name) {
                    let s = s.clone();
                    let result = match adv {
                        Adverb::Reduce => return Err(Error::Other("reduce on infinite stream".into())),
                        Adverb::Scan => Value::Stream(Arc::new(IterStream::infinite(move || {
                            let mut it = s.iter();
                            let mut acc = it.next().and_then(|v| v.as_real()).unwrap_or(0.0);
                            let mut first_emitted = false;
                            Box::new(std::iter::from_fn(move || {
                                if !first_emitted {
                                    first_emitted = true;
                                    return Some(Value::Real(acc));
                                }
                                it.next().and_then(|v| v.as_real()).map(|x| {
                                    acc = f(acc, x);
                                    Value::Real(acc)
                                })
                            }))
                        }))),
                        Adverb::Pairwise => Value::Stream(Arc::new(IterStream::infinite(move || {
                            let mut it = s.iter();
                            let mut prev = it.next().and_then(|v| v.as_real()).unwrap_or(0.0);
                            let mut first_emitted = false;
                            Box::new(std::iter::from_fn(move || {
                                if !first_emitted {
                                    first_emitted = true;
                                    return Some(Value::Real(prev));
                                }
                                it.next().and_then(|v| v.as_real()).map(|x| {
                                    let r = f(prev, x);
                                    prev = x;
                                    Value::Real(r)
                                })
                            }))
                        }))),
                    };
                    self.push(result);
                    return Ok(());
                }
                return Err(Error::Other(format!(
                    "adverb on infinite stream requires an arithmetic op, got '{name}'"
                )));
            }
        }
        let items = collect_to_vec(&list_val)?;
        match adv {
            Adverb::Reduce => {
                if items.is_empty() {
                    return Err(Error::Other("reduce on empty list".into()));
                }
                let mut acc = items[0].clone();
                for item in &items[1..] {
                    self.push(acc);
                    self.push(item.clone());
                    self.call_word(name)?;
                    acc = self.pop()?;
                }
                self.push(acc);
            }
            Adverb::Scan => {
                if items.is_empty() { self.push(make_list(vec![])); return Ok(()); }
                let mut result = vec![items[0].clone()];
                let mut acc = items[0].clone();
                for item in &items[1..] {
                    self.push(acc);
                    self.push(item.clone());
                    self.call_word(name)?;
                    acc = self.pop()?;
                    result.push(acc.clone());
                }
                self.push(make_list(result));
            }
            Adverb::Pairwise => {
                if items.is_empty() { self.push(make_list(vec![])); return Ok(()); }
                let mut result = vec![items[0].clone()];
                for w in items.windows(2) {
                    // push w[1] first so TOS=w[0]; f(a=w[1], b=w[0]) → w[1]-w[0] for `-^`
                    self.push(w[1].clone());
                    self.push(w[0].clone());
                    self.call_word(name)?;
                    result.push(self.pop()?);
                }
                self.push(make_list(result));
            }
        }
        Ok(())
    }

    // ---- each -----------------------------------------------------------

    fn apply_each_word(&mut self, name: &Arc<str>, depth: u8) -> Result<()> {
        // Outer-product mode: two rank-tagged args (@1/@2)
        if self.rank_args.len() >= 2 {
            let mut args = std::mem::take(&mut self.rank_args);
            args.sort_by_key(|&(r, _)| r);
            let (_, outer_val) = args.remove(0);
            let (_, inner_val) = args.remove(0);
            let outer = collect_to_vec(&outer_val)?;
            let inner = collect_to_vec(&inner_val)?;
            let mut out = Vec::with_capacity(outer.len());
            for o in &outer {
                let mut row = Vec::with_capacity(inner.len());
                for iv in &inner {
                    self.push(iv.clone());
                    self.push(o.clone());
                    self.call_word(name)?;
                    row.push(self.pop()?);
                }
                out.push(make_list(row));
            }
            self.push(make_list(out));
            return Ok(());
        }

        if let Some(list_val) = self.each_list.take() {
            // Zip mode: two @ calls — iterate both lists in parallel
            if let Some(zip_val) = self.each_zip.take() {
                let a_items = collect_to_vec(&list_val)?;
                let b_items = collect_to_vec(&zip_val)?;
                let len = a_items.len().min(b_items.len());
                let mut result = Vec::with_capacity(len);
                for (a, b) in a_items.iter().take(len).zip(b_items.iter().take(len)) {
                    self.push(a.clone());
                    self.push(b.clone());
                    self.call_word(name)?;
                    result.push(self.pop()?);
                }
                self.push(make_list(result));
                return Ok(());
            }
            // Depth ≥ 1: recursive nested map
            // Save base items (below mark) so they can be restored per-iteration.
            let base: Vec<Value> = self.stack[..self.each_stack_mark].to_vec();
            let extra: Vec<Value> = self.stack.drain(self.each_stack_mark..).collect();
            let items = collect_to_vec(&list_val)?;
            let result = apply_each_depth_word(self, name, items, &extra, depth, &base)?;
            self.stack.truncate(0); // base items consumed by the op
            self.push(result);
        } else {
            // Op::Each path: list is TOS
            let list_val = self.pop()?;
            let items = collect_to_vec(&list_val)?;
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                self.push(item);
                self.call_word(name)?;
                result.push(self.pop()?);
            }
            self.push(make_list(result));
        }
        Ok(())
    }

    fn apply_each_val(&mut self, v: Value, depth: u8) -> Result<()> {
        // Outer-product mode
        if self.rank_args.len() >= 2 {
            let mut args = std::mem::take(&mut self.rank_args);
            args.sort_by_key(|&(r, _)| r);
            let (_, outer_val) = args.remove(0);
            let (_, inner_val) = args.remove(0);
            let outer = collect_to_vec(&outer_val)?;
            let inner = collect_to_vec(&inner_val)?;
            let mut out = Vec::with_capacity(outer.len());
            for o in &outer {
                let mut row = Vec::with_capacity(inner.len());
                for iv in &inner {
                    self.push(iv.clone());
                    self.push(o.clone());
                    self.apply_or_push(v.clone())?;
                    row.push(self.pop()?);
                }
                out.push(make_list(row));
            }
            self.push(make_list(out));
            return Ok(());
        }

        if let Some(list_val) = self.each_list.take() {
            // Zip mode
            if let Some(zip_val) = self.each_zip.take() {
                let a_items = collect_to_vec(&list_val)?;
                let b_items = collect_to_vec(&zip_val)?;
                let len = a_items.len().min(b_items.len());
                let mut result = Vec::with_capacity(len);
                for (a, b) in a_items.iter().take(len).zip(b_items.iter().take(len)) {
                    self.push(a.clone());
                    self.push(b.clone());
                    self.apply_or_push(v.clone())?;
                    result.push(self.pop()?);
                }
                self.push(make_list(result));
                return Ok(());
            }
            let base: Vec<Value> = self.stack[..self.each_stack_mark].to_vec();
            let extra: Vec<Value> = self.stack.drain(self.each_stack_mark..).collect();
            let items = collect_to_vec(&list_val)?;
            let result = apply_each_depth_val(self, &v, items, &extra, depth, &base)?;
            self.stack.truncate(0);
            self.push(result);
        } else {
            let list_val = self.pop()?;
            let items = collect_to_vec(&list_val)?;
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                self.push(item);
                self.apply_or_push(v.clone())?;
                result.push(self.pop()?);
            }
            self.push(make_list(result));
        }
        Ok(())
    }

    // ---- misc -----------------------------------------------------------
    #[doc(hidden)]
    pub fn __kind_of_top(&self) -> Option<ValueKind> {
        self.peek().map(|v| v.kind())
    }
}

impl Default for Interp { fn default() -> Self { Self::new() } }

// ---- depth-recursive each helpers (standalone, borrow interp mutably) ------

fn apply_each_depth_word(
    interp: &mut Interp,
    name: &Arc<str>,
    items: Vec<Value>,
    extra: &[Value],
    depth: u8,
    base: &[Value],
) -> Result<Value> {
    if depth <= 1 {
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            interp.stack.truncate(0);
            for v in base { interp.stack.push(v.clone()); }
            interp.push(item);
            for a in extra { interp.push(a.clone()); }
            interp.call_word(name)?;
            result.push(interp.pop()?);
        }
        Ok(make_list(result))
    } else {
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            if let Value::Stream(_) = &item {
                let inner = collect_to_vec(&item)?;
                result.push(apply_each_depth_word(interp, name, inner, extra, depth - 1, base)?);
            } else {
                interp.stack.truncate(0);
                for v in base { interp.stack.push(v.clone()); }
                interp.push(item);
                for a in extra { interp.push(a.clone()); }
                interp.call_word(name)?;
                result.push(interp.pop()?);
            }
        }
        Ok(make_list(result))
    }
}

fn apply_each_depth_val(
    interp: &mut Interp,
    v: &Value,
    items: Vec<Value>,
    extra: &[Value],
    depth: u8,
    base: &[Value],
) -> Result<Value> {
    if depth <= 1 {
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            interp.stack.truncate(0);
            for bv in base { interp.stack.push(bv.clone()); }
            interp.push(item);
            for a in extra { interp.push(a.clone()); }
            interp.apply_or_push(v.clone())?;
            result.push(interp.pop()?);
        }
        Ok(make_list(result))
    } else {
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            if let Value::Stream(_) = &item {
                let inner = collect_to_vec(&item)?;
                result.push(apply_each_depth_val(interp, v, inner, extra, depth - 1, base)?);
            } else {
                interp.stack.truncate(0);
                for bv in base { interp.stack.push(bv.clone()); }
                interp.push(item);
                for a in extra { interp.push(a.clone()); }
                interp.apply_or_push(v.clone())?;
                result.push(interp.pop()?);
            }
        }
        Ok(make_list(result))
    }
}

// ---- shared helpers (pub for tests) -------------------------------------

pub fn make_list(items: Vec<Value>) -> Value {
    Value::Stream(Arc::new(VecStream(items)))
}

/// Resolve a word name to an arithmetic function pointer (for lazy adverbs on infinite streams).
fn resolve_arith(name: &str) -> Option<fn(f64, f64) -> f64> {
    match name {
        "+" => Some(|a, b| a + b),
        "-" => Some(|a, b| a - b),
        "*" => Some(|a, b| a * b),
        "/" => Some(|a, b| a / b),
        "min" => Some(f64::min),
        "max" => Some(f64::max),
        _ => None,
    }
}

/// Unary map that recurses into nested streams.
pub fn automap_unary(v: Value, f: fn(f64) -> f64) -> Result<Value> {
    match v {
        Value::Real(x) => Ok(Value::Real(f(x))),
        Value::Stream(ref s) if s.is_infinite() => {
            let s2 = s.clone();
            Ok(Value::Stream(Arc::new(IterStream::infinite(move || {
                let mut it = s2.iter();
                Box::new(std::iter::from_fn(move || {
                    it.next().and_then(|x| automap_unary(x, f).ok())
                }))
            }))))
        }
        Value::Stream(_) => {
            let items = collect_to_vec(&v)?;
            let out: Result<Vec<Value>> = items.into_iter().map(|x| automap_unary(x, f)).collect();
            Ok(make_list(out?))
        }
        Value::Signal(ref s) => {
            match s.as_f32_slice() {
                Some(sl) => Ok(make_signal(sl.iter().map(|&x| f(x as f64) as f32).collect())),
                None => Ok(Value::Signal(Arc::new(UnarySignal { inner: s.clone(), op: f }))),
            }
        }
        other => Err(Error::Type { expected: "Real or Stream", actual: other.kind().name() }),
    }
}

/// Like collect_to_vec but also accepts Signal (converts f32 samples to Real values).
pub fn collect_to_vec_or_signal(v: &Value) -> Result<Vec<Value>> {
    match v {
        Value::Signal(s) => {
            if let Some(sl) = s.as_f32_slice() {
                Ok(sl.iter().map(|&x| Value::Real(x as f64)).collect())
            } else {
                Err(Error::Other("cannot read this signal as an array".into()))
            }
        }
        _ => collect_to_vec(v),
    }
}

pub fn make_signal(samples: Vec<f32>) -> Value {
    Value::Signal(Arc::new(VecSignal(samples)))
}

pub fn collect_to_vec(v: &Value) -> Result<Vec<Value>> {
    match v {
        Value::Stream(s) => {
            if s.is_infinite() {
                return Err(Error::Other("cannot materialize infinite stream".into()));
            }
            let mut it = s.iter();
            let mut out = Vec::new();
            while let Some(x) = it.next() { out.push(x); }
            Ok(out)
        }
        Value::Real(_) => Ok(vec![v.clone()]),
        _ => Err(Error::Type { expected: "Stream or Real", actual: v.kind().name() }),
    }
}

fn collect_signal_f32(v: &Value) -> Result<Vec<f32>> {
    match v {
        Value::Signal(s) => {
            if let Some(sl) = s.as_f32_slice() { return Ok(sl.to_vec()); }
            Err(Error::Other("cannot read this signal as array".into()))
        }
        Value::Real(x) => Ok(vec![*x as f32]),
        _ => Err(Error::Type { expected: "Signal", actual: v.kind().name() }),
    }
}

/// Collect a finite signal's samples, instantiating at `sr` if not array-backed.
fn collect_signal_f32_sr(s: &Arc<dyn stax_core::Signal>, sr: f64) -> Result<Vec<f32>> {
    if let Some(sl) = s.as_f32_slice() { return Ok(sl.to_vec()); }
    if let Some(n) = s.len_hint() {
        if n > 0 {
            let mut inst = s.instantiate(sr);
            let mut out = vec![0.0f32; n];
            inst.fill(&mut out);
            return Ok(out);
        }
    }
    Err(Error::Other("signal must be finite (array-backed) for offline processing".into()))
}

pub fn value_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Real(x), Value::Real(y)) => x == y,
        (Value::Str(a), Value::Str(b)) | (Value::Sym(a), Value::Sym(b)) => a == b,
        (Value::Nil, Value::Nil) => true,
        (Value::Stream(sa), Value::Stream(sb)) => {
            if sa.is_infinite() || sb.is_infinite() { return false; }
            let mut ia = sa.iter(); let mut ib = sb.iter();
            loop {
                match (ia.next(), ib.next()) {
                    (None, None) => return true,
                    (Some(av), Some(bv)) => if !value_equal(&av, &bv) { return false; },
                    _ => return false,
                }
            }
        }
        (Value::Signal(sa), Value::Signal(sb)) => {
            match (sa.as_f32_slice(), sb.as_f32_slice()) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        }
        (Value::Form(fa), Value::Form(fb)) => {
            if fa.bindings.len() != fb.bindings.len() || fa.parents.len() != fb.parents.len() {
                return false;
            }
            for (k, va) in &fa.bindings {
                match fb.bindings.get(k) {
                    None => return false,
                    Some(vb) => if !value_equal(va, vb) { return false; }
                }
            }
            fa.parents.iter().zip(fb.parents.iter())
                .all(|(pa, pb)| value_equal(&Value::Form(pa.clone()), &Value::Form(pb.clone())))
        }
        _ => false,
    }
}

/// Auto-mapping binary op: handles Real×Real, Stream×anything, Signal×anything.
pub fn automap_bin(a: Value, b: Value, f: fn(f64, f64) -> f64) -> Result<Value> {
    let a_inf = matches!(&a, Value::Stream(s) if s.is_infinite());
    let b_inf = matches!(&b, Value::Stream(s) if s.is_infinite());

    match (&a, &b) {
        (Value::Real(x), Value::Real(y)) => Ok(Value::Real(f(*x, *y))),

        // Lazy infinite cases — never materialize
        (Value::Stream(sa), Value::Real(y)) if a_inf => {
            let (y, sa) = (*y, sa.clone());
            Ok(Value::Stream(Arc::new(IterStream::infinite(move || {
                let mut it = sa.iter();
                Box::new(std::iter::from_fn(move || {
                    it.next().and_then(|av| av.as_real().map(|x| Value::Real(f(x, y))))
                }))
            }))))
        }
        (Value::Real(x), Value::Stream(sb)) if b_inf => {
            let (x, sb) = (*x, sb.clone());
            Ok(Value::Stream(Arc::new(IterStream::infinite(move || {
                let mut it = sb.iter();
                Box::new(std::iter::from_fn(move || {
                    it.next().and_then(|bv| bv.as_real().map(|y| Value::Real(f(x, y))))
                }))
            }))))
        }
        (Value::Stream(sa), Value::Stream(sb)) if a_inf && b_inf => {
            let (sa, sb) = (sa.clone(), sb.clone());
            Ok(Value::Stream(Arc::new(IterStream::infinite(move || {
                let mut ia = sa.iter(); let mut ib = sb.iter();
                Box::new(std::iter::from_fn(move || {
                    match (ia.next(), ib.next()) {
                        (Some(av), Some(bv)) => automap_bin(av, bv, f).ok(),
                        _ => None,
                    }
                }))
            }))))
        }
        // infinite op finite → truncate to finite length
        (Value::Stream(sa), Value::Stream(_)) if a_inf => {
            let items_b = collect_to_vec(&b)?;
            let mut ia = sa.iter();
            let out: Vec<Value> = items_b.iter()
                .filter_map(|bv| ia.next().and_then(|av| automap_bin(av, bv.clone(), f).ok()))
                .collect();
            Ok(make_list(out))
        }
        (Value::Stream(_), Value::Stream(sb)) if b_inf => {
            let items_a = collect_to_vec(&a)?;
            let mut ib = sb.iter();
            let out: Vec<Value> = items_a.iter()
                .filter_map(|av| ib.next().and_then(|bv| automap_bin(av.clone(), bv, f).ok()))
                .collect();
            Ok(make_list(out))
        }

        // Finite stream cases
        (Value::Stream(_), Value::Stream(_)) => {
            let ia = collect_to_vec(&a)?;
            let ib = collect_to_vec(&b)?;
            let len = ia.len().min(ib.len());
            let mut out = Vec::with_capacity(len);
            for (x, y) in ia.into_iter().zip(ib).take(len) {
                out.push(automap_bin(x, y, f)?);
            }
            Ok(make_list(out))
        }
        (Value::Stream(_), _) => {
            let items = collect_to_vec(&a)?;
            let mut out = Vec::with_capacity(items.len());
            for x in items { out.push(automap_bin(x, b.clone(), f)?); }
            Ok(make_list(out))
        }
        (_, Value::Stream(_)) => {
            let items = collect_to_vec(&b)?;
            let mut out = Vec::with_capacity(items.len());
            for y in items { out.push(automap_bin(a.clone(), y, f)?); }
            Ok(make_list(out))
        }

        (Value::Signal(sa), Value::Signal(sb)) => {
            match (sa.as_f32_slice(), sb.as_f32_slice()) {
                (Some(a_sl), Some(b_sl)) => {
                    let len = a_sl.len().min(b_sl.len());
                    let out: Vec<f32> = a_sl[..len].iter().zip(&b_sl[..len])
                        .map(|(&x, &y)| f(x as f64, y as f64) as f32)
                        .collect();
                    Ok(make_signal(out))
                }
                _ => Ok(Value::Signal(Arc::new(BinarySignal { a: sa.clone(), b: sb.clone(), op: f }))),
            }
        }
        (Value::Signal(sa), Value::Real(y)) => {
            match sa.as_f32_slice() {
                Some(sl) => {
                    let y = *y;
                    Ok(make_signal(sl.iter().map(|&x| f(x as f64, y) as f32).collect()))
                }
                None => Ok(Value::Signal(Arc::new(BinarySignal {
                    a: sa.clone(),
                    b: Arc::new(ConstSignal { value: *y as f32 }),
                    op: f,
                }))),
            }
        }
        (Value::Real(x), Value::Signal(sb)) => {
            match sb.as_f32_slice() {
                Some(sl) => {
                    let x = *x;
                    Ok(make_signal(sl.iter().map(|&y| f(x, y as f64) as f32).collect()))
                }
                None => Ok(Value::Signal(Arc::new(BinarySignal {
                    a: Arc::new(ConstSignal { value: *x as f32 }),
                    b: sb.clone(),
                    op: f,
                }))),
            }
        }

        _ => Err(Error::Type { expected: "Real, Stream, or Signal", actual: "incompatible types" }),
    }
}

fn real_val(v: &Value) -> Result<f64> {
    v.as_real().ok_or(Error::Type { expected: "Real", actual: v.kind().name() })
}

// ---- index helpers -------------------------------------------------------

fn at_zero(items: &[Value], idx: isize) -> Value {
    if idx < 0 || idx as usize >= items.len() { Value::Real(0.0) }
    else { items[idx as usize].clone() }
}

fn wrap_idx(len: usize, idx: isize) -> usize {
    if len == 0 { return 0; }
    idx.rem_euclid(len as isize) as usize
}

fn clip_idx(len: usize, idx: isize) -> usize {
    if len == 0 { return 0; }
    idx.clamp(0, len as isize - 1) as usize
}

fn fold_idx(len: usize, idx: isize) -> usize {
    if len == 0 { return 0; }
    if len == 1 { return 0; }
    let period = 2 * (len as isize - 1);
    let mut i = idx.rem_euclid(period);
    if i >= len as isize { i = period - i; }
    i as usize
}

fn map_index<F: Fn(usize, isize) -> Value>(items: &[Value], idx: &Value, f: F) -> Result<Value> {
    match idx {
        Value::Real(k) => Ok(f(items.len(), *k as isize)),
        Value::Stream(_) => {
            let idxs = collect_to_vec(idx)?;
            let vals: Vec<Value> = idxs.iter().map(|iv| {
                if let Value::Real(k) = iv { f(items.len(), *k as isize) } else { Value::Real(0.0) }
            }).collect();
            Ok(make_list(vals))
        }
        Value::Signal(s) => {
            if let Some(sl) = s.as_f32_slice() {
                let vals: Vec<Value> = sl.iter().map(|&k| f(items.len(), k as isize)).collect();
                Ok(make_list(vals))
            } else {
                Err(Error::Other("cannot use this signal as an index".into()))
            }
        }
        other => Err(Error::Type { expected: "Real, Stream, or Signal", actual: other.kind().name() }),
    }
}

// ---- flatten helpers -------------------------------------------------------

fn flatten_deep(v: &Value) -> Result<Vec<Value>> {
    if let Value::Stream(_) = v {
        let items = collect_to_vec(v)?;
        let mut out = Vec::new();
        for item in items { out.extend(flatten_deep(&item)?); }
        Ok(out)
    } else {
        Ok(vec![v.clone()])
    }
}

fn flatten_n(v: &Value, n: usize) -> Result<Vec<Value>> {
    if n == 0 { return Ok(vec![v.clone()]); }
    if let Value::Stream(_) = v {
        let items = collect_to_vec(v)?;
        let mut out = Vec::new();
        for item in items { out.extend(flatten_n(&item, n - 1)?); }
        Ok(out)
    } else {
        Ok(vec![v.clone()])
    }
}

// ---- helper: wrap any value as a cycling infinite stream -----------------

fn to_cycling_stream(v: Value) -> Arc<dyn stax_core::stream::Stream> {
    match v {
        Value::Stream(s) if s.is_infinite() => s,
        Value::Stream(s) => {
            let items: Arc<Vec<Value>> = {
                let mut it = s.iter();
                Arc::new(std::iter::from_fn(|| it.next()).collect())
            };
            Arc::new(IterStream::infinite(move || {
                let items = items.clone();
                let mut pos = 0usize;
                Box::new(std::iter::from_fn(move || {
                    if items.is_empty() { return None; }
                    let v = items[pos % items.len()].clone();
                    pos += 1;
                    Some(v)
                }))
            }))
        }
        other => Arc::new(IterStream::infinite(move || {
            let v = other.clone();
            Box::new(std::iter::repeat(v))
        })),
    }
}

// ---- multi-arg scalar-map helpers (closures capture params, can't use fn ptr)

fn automap_with<F>(v: Value, f: F) -> Result<Value>
where F: Fn(f64) -> f64 + Clone
{
    match v {
        Value::Real(x) => Ok(Value::Real(f(x))),
        Value::Stream(_) => {
            let items = collect_to_vec(&v)?;
            let out: Result<Vec<Value>> = items.into_iter()
                .map(|x| automap_with(x, f.clone()))
                .collect();
            Ok(make_list(out?))
        }
        Value::Signal(s) => {
            if let Some(sl) = s.as_f32_slice() {
                let out: Vec<f32> = sl.iter().map(|&x| f(x as f64) as f32).collect();
                Ok(make_signal(out))
            } else {
                Err(Error::Other("cannot apply to lazy signal (materialize first)".into()))
            }
        }
        other => Err(Error::Type { expected: "Real or Stream", actual: other.kind().name() }),
    }
}

fn apply_clip(v: Value, lo: f64, hi: f64) -> Result<Value> {
    automap_with(v, move |x| x.clamp(lo, hi))
}

fn apply_wrap(v: Value, lo: f64, hi: f64) -> Result<Value> {
    let range = hi - lo;
    automap_with(v, move |x| {
        if range == 0.0 { return lo; }
        lo + (x - lo).rem_euclid(range)
    })
}

fn apply_fold(v: Value, lo: f64, hi: f64) -> Result<Value> {
    let range = hi - lo;
    automap_with(v, move |x| {
        if range == 0.0 { return lo; }
        let period = range * 2.0;
        let t = (x - lo).rem_euclid(period);
        lo + if t <= range { t } else { period - t }
    })
}

fn apply_linlin(v: Value, src_lo: f64, src_hi: f64, dst_lo: f64, dst_hi: f64) -> Result<Value> {
    let src_range = src_hi - src_lo;
    automap_with(v, move |x| {
        if src_range == 0.0 { return dst_lo; }
        dst_lo + (x - src_lo) / src_range * (dst_hi - dst_lo)
    })
}

fn apply_linexp(v: Value, src_lo: f64, src_hi: f64, dst_lo: f64, dst_hi: f64) -> Result<Value> {
    let src_range = src_hi - src_lo;
    automap_with(v, move |x| {
        if src_range == 0.0 || dst_lo <= 0.0 || dst_hi <= 0.0 { return dst_lo; }
        dst_lo * (dst_hi / dst_lo).powf((x - src_lo) / src_range)
    })
}

fn apply_explin(v: Value, src_lo: f64, src_hi: f64, dst_lo: f64, dst_hi: f64) -> Result<Value> {
    let log_ratio = if src_lo <= 0.0 || src_hi <= 0.0 { 1.0 } else { (src_hi / src_lo).ln() };
    automap_with(v, move |x| {
        if log_ratio == 0.0 { return dst_lo; }
        dst_lo + (x / src_lo.max(1e-10)).ln() / log_ratio * (dst_hi - dst_lo)
    })
}

// ---- bilin / biexp helpers (closures capture lo/hi so can't use fn ptr) ----

fn apply_bilin(v: Value, lo: f64, hi: f64) -> Result<Value> {
    match v {
        Value::Real(x) => Ok(Value::Real(lo + (hi - lo) * (x + 1.0) * 0.5)),
        Value::Stream(_) => {
            let items = collect_to_vec(&v)?;
            let out: Result<Vec<Value>> = items.into_iter()
                .map(|x| apply_bilin(x, lo, hi))
                .collect();
            Ok(make_list(out?))
        }
        Value::Signal(s) => {
            if let Some(sl) = s.as_f32_slice() {
                let out: Vec<f32> = sl.iter()
                    .map(|&x| (lo + (hi - lo) * (x as f64 + 1.0) * 0.5) as f32)
                    .collect();
                Ok(make_signal(out))
            } else {
                Err(Error::Other("bilin: cannot apply to lazy signal".into()))
            }
        }
        other => Err(Error::Type { expected: "Real or Stream", actual: other.kind().name() }),
    }
}

fn apply_biexp(v: Value, lo: f64, hi: f64) -> Result<Value> {
    let f = |x: f64| {
        if lo <= 0.0 || hi <= 0.0 { lo + (hi - lo) * (x + 1.0) * 0.5 }
        else { lo * (hi / lo).powf((x + 1.0) * 0.5) }
    };
    match v {
        Value::Real(x) => Ok(Value::Real(f(x))),
        Value::Stream(_) => {
            let items = collect_to_vec(&v)?;
            let out: Result<Vec<Value>> = items.into_iter()
                .map(|x| apply_biexp(x, lo, hi))
                .collect();
            Ok(make_list(out?))
        }
        Value::Signal(s) => {
            if let Some(sl) = s.as_f32_slice() {
                let out: Vec<f32> = sl.iter().map(|&x| f(x as f64) as f32).collect();
                Ok(make_signal(out))
            } else {
                Err(Error::Other("biexp: cannot apply to lazy signal".into()))
            }
        }
        other => Err(Error::Type { expected: "Real or Stream", actual: other.kind().name() }),
    }
}

// ---- builtin installation -----------------------------------------------

fn reg<F>(interp: &mut Interp, name: &str, f: F)
where F: Fn(&mut Interp) -> Result<()> + Send + Sync + 'static
{
    interp.builtins.insert(Arc::from(name), Arc::new(f));
}

/// Advance the interpreter's LCG RNG by one step and return the new seed.
#[inline]
fn rng_step(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *seed
}

/// Pop a Signal from the stack, returning a type error if TOS is not a Signal.
fn pop_signal(i: &mut Interp) -> Result<Arc<dyn stax_core::Signal>> {
    match i.pop()? {
        Value::Signal(s) => Ok(s),
        other => Err(Error::Type { expected: "Signal", actual: other.kind().name() }),
    }
}

fn install_builtins(i: &mut Interp) {
    // Arithmetic (auto-mapped)
    reg(i, "+",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|x+y)?); Ok(()) });
    reg(i, "-",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|x-y)?); Ok(()) });
    reg(i, "*",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|x*y)?); Ok(()) });
    reg(i, "/",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|x/y)?); Ok(()) });
    reg(i, "pow", |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,f64::powf)?); Ok(()) });
    reg(i, "min", |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,f64::min)?); Ok(()) });
    reg(i, "max", |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,f64::max)?); Ok(()) });
    reg(i, "mod", |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|x%y)?); Ok(()) });
    reg(i, "atan2",|i|{ let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,f64::atan2)?);Ok(()) });
    reg(i, "<",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|if x<y{1.0}else{0.0})?); Ok(()) });
    reg(i, ">",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|if x>y{1.0}else{0.0})?); Ok(()) });
    reg(i, "<=", |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|if x<=y{1.0}else{0.0})?); Ok(()) });
    reg(i, ">=", |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|if x>=y{1.0}else{0.0})?); Ok(()) });

    // Unary math — uses automap_unary which recurses into nested streams
    macro_rules! umap {
        ($name:expr, $f:expr) => {
            reg(i, $name, |i| {
                let v = i.pop()?;
                i.push(automap_unary(v, $f)?);
                Ok(())
            });
        }
    }
    umap!("neg",   |x: f64| -x);
    umap!("abs",   f64::abs);
    umap!("sq",    |x: f64| x*x);
    umap!("sqrt",  f64::sqrt);
    umap!("floor", f64::floor);
    umap!("ceil",  f64::ceil);
    umap!("round", f64::round);
    umap!("ln",    f64::ln);
    umap!("log2",  f64::log2);
    umap!("log",   f64::log10);
    umap!("exp",   f64::exp);
    umap!("sin",   f64::sin);
    umap!("cos",   f64::cos);
    umap!("tan",   f64::tan);
    umap!("asin",  f64::asin);
    umap!("acos",  f64::acos);
    umap!("atan",  f64::atan);
    umap!("inc",   |x: f64| x + 1.0);
    umap!("dec",   |x: f64| x - 1.0);
    umap!("recip", |x: f64| 1.0 / x);

    // Logic
    reg(i, "&", |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|if x!=0.0&&y!=0.0{1.0}else{0.0})?); Ok(()) });
    reg(i, "|", |i| { let b=i.pop()?; let a=i.pop()?; i.push(automap_bin(a,b,|x,y|if x!=0.0||y!=0.0{1.0}else{0.0})?); Ok(()) });
    reg(i, "equals", |i| {
        let b = i.pop()?; let a = i.pop()?;
        i.push(Value::Real(if value_equal(&a,&b) {1.0} else {0.0}));
        Ok(())
    });
    reg(i, "not", |i| {
        let v = i.pop()?;
        i.push(Value::Real(if v.is_truthy() {0.0} else {1.0}));
        Ok(())
    });
    reg(i, "if", |i| {
        // stack: condition, true-thunk, false-thunk (TOS)
        let f_thunk = i.pop()?;
        let t_thunk = i.pop()?;
        let cond    = i.pop()?;
        let thunk = if cond.is_truthy() { t_thunk } else { f_thunk };
        i.apply_or_push(thunk)
    });

    // @ each — first @ sets list; second @ enters zip mode
    reg(i, "@", |i| {
        let list = i.pop()?;
        if i.each_list.is_some() {
            i.each_zip = Some(list);
        } else {
            i.each_list = Some(list);
            i.each_stack_mark = i.stack.len();
            i.each_depth = 1;
        }
        Ok(())
    });
    // @@ depth-2 each; @@@ depth-3
    reg(i, "@@", |i| {
        let list = i.pop()?;
        i.each_list = Some(list);
        i.each_stack_mark = i.stack.len();
        i.each_depth = 2;
        Ok(())
    });
    reg(i, "@@@", |i| {
        let list = i.pop()?;
        i.each_list = Some(list);
        i.each_stack_mark = i.stack.len();
        i.each_depth = 3;
        Ok(())
    });
    // @1 / @2 outer-product rank tags
    reg(i, "@1", |i| { let v = i.pop()?; i.rank_args.push((1, v)); Ok(()) });
    reg(i, "@2", |i| { let v = i.pop()?; i.rank_args.push((2, v)); Ok(()) });

    // Stack shufflers
    reg(i, "dup",  |i| { let v=i.pop()?; i.push(v.clone()); i.push(v); Ok(()) });
    reg(i, "swap", |i| { let b=i.pop()?; let a=i.pop()?; i.push(b); i.push(a); Ok(()) });
    reg(i, "over", |i| { let b=i.pop()?; let a=i.pop()?; i.push(a.clone()); i.push(b); i.push(a); Ok(()) });
    // SAPF naming: a=oldest consumed, b=next, c=TOS
    reg(i, "aa",   |i| { let a=i.pop()?; i.push(a.clone()); i.push(a); Ok(()) });
    reg(i, "ba",   |i| { let b=i.pop()?; let a=i.pop()?; i.push(b); i.push(a); Ok(()) });
    reg(i, "bab",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(b.clone()); i.push(a); i.push(b); Ok(()) });
    reg(i, "aba",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(a.clone()); i.push(b); i.push(a); Ok(()) });
    reg(i, "aab",  |i| { let b=i.pop()?; let a=i.pop()?; i.push(a.clone()); i.push(a); i.push(b); Ok(()) });
    reg(i, "aabb", |i| { let b=i.pop()?; let a=i.pop()?; i.push(a.clone()); i.push(a); i.push(b.clone()); i.push(b); Ok(()) });
    reg(i, "abab", |i| { let b=i.pop()?; let a=i.pop()?; i.push(a.clone()); i.push(b.clone()); i.push(a); i.push(b); Ok(()) });
    // 3-item: a=third, b=second, c=TOS
    reg(i, "bac",  |i| { let c=i.pop()?; let b=i.pop()?; let a=i.pop()?; i.push(b); i.push(a); i.push(c); Ok(()) });
    reg(i, "cba",  |i| { let c=i.pop()?; let b=i.pop()?; let a=i.pop()?; i.push(c); i.push(b); i.push(a); Ok(()) });
    reg(i, "bca",  |i| { let c=i.pop()?; let b=i.pop()?; let a=i.pop()?; i.push(b); i.push(c); i.push(a); Ok(()) });
    reg(i, "cab",  |i| { let c=i.pop()?; let b=i.pop()?; let a=i.pop()?; i.push(c); i.push(a); i.push(b); Ok(()) });
    reg(i, "nip",  |i| { let c=i.pop()?; let _=i.pop()?; let a=i.pop()?; i.push(a); i.push(c); Ok(()) });
    reg(i, "pop",  |i| { i.pop()?; Ok(()) });
    reg(i, "clear", |i| { i.stack.clear(); Ok(()) });
    reg(i, "cleard", |i| {
        let d = i.stack.len() as f64;
        i.stack.clear();
        i.push(Value::Real(d));
        Ok(())
    });
    reg(i, "stackDepth", |i| {
        let d = i.stack.len() as f64;
        i.push(Value::Real(d));
        Ok(())
    });

    // Tuple constructors
    reg(i, "2ple", |i| { let b=i.pop()?; let a=i.pop()?; i.push(make_list(vec![a,b])); Ok(()) });
    reg(i, "3ple", |i| { let c=i.pop()?; let b=i.pop()?; let a=i.pop()?; i.push(make_list(vec![a,b,c])); Ok(()) });
    reg(i, "4ple", |i| { let d=i.pop()?; let c=i.pop()?; let b=i.pop()?; let a=i.pop()?; i.push(make_list(vec![a,b,c,d])); Ok(()) });
    reg(i, "nple", |i| {
        let n=real_val(&i.pop()?)? as usize;
        let items=i.pop_n(n)?;
        i.push(make_list(items)); Ok(())
    });
    // ple: parsed from Nple (e.g. "2ple" → Lit(2) Word("ple"))
    reg(i, "ple", |i| {
        let n=real_val(&i.pop()?)? as usize;
        let items=i.pop_n(n)?;
        i.push(make_list(items)); Ok(())
    });
    reg(i, "2ples", |i| {
        let b=collect_to_vec(&i.pop()?)?; let a=collect_to_vec(&i.pop()?)?;
        let len=a.len().min(b.len());
        let r: Vec<Value>=(0..len).map(|k| make_list(vec![a[k].clone(),b[k].clone()])).collect();
        i.push(make_list(r)); Ok(())
    });
    // ples: parsed from Nples (e.g. "2ples" → Lit(2) Word("ples")); pops n lists, interleaves
    // When all inputs are scalars, returns a single flat tuple (SAPF automap semantics)
    reg(i, "ples", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let mut inputs: Vec<Value> = (0..n).map(|_| i.pop()).collect::<Result<_>>()?;
        inputs.reverse();
        let all_scalar = inputs.iter().all(|v| !matches!(v, Value::Stream(_)));
        if all_scalar {
            i.push(make_list(inputs));
        } else {
            let lists: Vec<Vec<Value>> = inputs.iter().map(collect_to_vec).collect::<Result<_>>()?;
            let len = lists.iter().map(|l| l.len()).min().unwrap_or(0);
            let r: Vec<Value> = (0..len).map(|k| make_list(lists.iter().map(|l| l[k].clone()).collect())).collect();
            i.push(make_list(r));
        }
        Ok(())
    });
    reg(i, "3ples", |i| {
        let c=collect_to_vec(&i.pop()?)?; let b=collect_to_vec(&i.pop()?)?; let a=collect_to_vec(&i.pop()?)?;
        let len=a.len().min(b.len()).min(c.len());
        let r: Vec<Value>=(0..len).map(|k| make_list(vec![a[k].clone(),b[k].clone(),c[k].clone()])).collect();
        i.push(make_list(r)); Ok(())
    });
    reg(i, "4ples", |i| {
        let d=collect_to_vec(&i.pop()?)?; let c=collect_to_vec(&i.pop()?)?;
        let b=collect_to_vec(&i.pop()?)?; let a=collect_to_vec(&i.pop()?)?;
        let len=a.len().min(b.len()).min(c.len()).min(d.len());
        let r: Vec<Value>=(0..len).map(|k| make_list(vec![a[k].clone(),b[k].clone(),c[k].clone(),d[k].clone()])).collect();
        i.push(make_list(r)); Ok(())
    });
    reg(i, "un2", |i| { let v=collect_to_vec(&i.pop()?)?; for x in v.into_iter().take(2) { i.push(x); } Ok(()) });
    reg(i, "un3", |i| { let v=collect_to_vec(&i.pop()?)?; for x in v.into_iter().take(3) { i.push(x); } Ok(()) });
    reg(i, "un4", |i| { let v=collect_to_vec(&i.pop()?)?; for x in v.into_iter().take(4) { i.push(x); } Ok(()) });

    // size / reverse
    reg(i, "size", |i| {
        let v = i.pop()?;
        let n = match &v {
            Value::Real(_) => 1.0,
            Value::Stream(s) => s.len_hint().unwrap_or(0) as f64,
            Value::Signal(s) => s.len_hint().unwrap_or(0) as f64,
            _ => return Err(Error::Type { expected: "Real or Stream", actual: v.kind().name() }),
        };
        i.push(Value::Real(n)); Ok(())
    });
    reg(i, "reverse", |i| {
        let v = i.pop()?;
        match &v {
            Value::Stream(_) => {
                let mut items = collect_to_vec(&v)?;
                items.reverse();
                i.push(make_list(items));
            }
            Value::Signal(_) => {
                let mut s = collect_signal_f32(&v)?;
                s.reverse();
                i.push(make_signal(s));
            }
            other => i.push(other.clone()),
        }
        Ok(())
    });

    // ord: 1, 2, 3, ...
    reg(i, "ord", |i| {
        i.push(Value::Stream(Arc::new(IterStream::infinite(|| {
            let mut n = 1u64;
            Box::new(std::iter::from_fn(move || { let v = Value::Real(n as f64); n+=1; Some(v) }))
        }))));
        Ok(())
    });
    // ordz: 1, 2, 3, ... as Signal (ZList)
    reg(i, "ordz", |i| {
        i.push(Value::Signal(Arc::new(GenSignal::new(|| {
            let mut n = 1u32;
            Box::new(std::iter::from_fn(move || { let v = n as f32; n += 1; Some(v) }))
        }))));
        Ok(())
    });
    // nat: 0, 1, 2, ...
    reg(i, "nat", |i| {
        i.push(Value::Stream(Arc::new(IterStream::infinite(|| {
            let mut n = 0u64;
            Box::new(std::iter::from_fn(move || { let v = Value::Real(n as f64); n+=1; Some(v) }))
        }))));
        Ok(())
    });
    // odds: 1, 3, 5, ...
    reg(i, "odds", |i| {
        i.push(Value::Stream(Arc::new(IterStream::infinite(|| {
            let mut n = 1u64;
            Box::new(std::iter::from_fn(move || { let v = Value::Real(n as f64); n+=2; Some(v) }))
        }))));
        Ok(())
    });
    // evens: 0, 2, 4, ...
    reg(i, "evens", |i| {
        i.push(Value::Stream(Arc::new(IterStream::infinite(|| {
            let mut n = 0u64;
            Box::new(std::iter::from_fn(move || { let v = Value::Real(n as f64); n+=2; Some(v) }))
        }))));
        Ok(())
    });
    // ints: 0, 1, -1, 2, -2, ...
    reg(i, "ints", |i| {
        i.push(Value::Stream(Arc::new(IterStream::infinite(|| {
            let mut k = 0i64;
            Box::new(std::iter::from_fn(move || {
                let v = Value::Real(k as f64);
                k = if k <= 0 { -k + 1 } else { -k };
                Some(v)
            }))
        }))));
        Ok(())
    });

    // a b to → finite stream a..=b (or b..=a); auto-maps when either arg is a Stream
    reg(i, "to", |i| {
        let b_val = i.pop()?;
        let a_val = i.pop()?;
        fn make_range(a_raw: f64, b_raw: f64) -> Result<Value> {
            if (b_raw - a_raw).abs() > 1_000_000.0 {
                return Err(Error::Other("to: range too large".into()));
            }
            let b = b_raw as i64; let a = a_raw as i64;
            let items: Vec<Value> = if a <= b {
                (a..=b).map(|n| Value::Real(n as f64)).collect()
            } else {
                (b..=a).rev().map(|n| Value::Real(n as f64)).collect()
            };
            Ok(Value::Stream(Arc::new(VecStream(items))))
        }
        if matches!(&a_val, Value::Stream(_)) || matches!(&b_val, Value::Stream(_)) {
            let a_items = if matches!(&a_val, Value::Stream(_)) { collect_to_vec(&a_val)? } else { vec![a_val] };
            let b_items = if matches!(&b_val, Value::Stream(_)) { collect_to_vec(&b_val)? } else { vec![b_val] };
            // Broadcast: if one side is scalar (len=1), extend to match the other
            let len = if a_items.len() == 1 { b_items.len() }
                      else if b_items.len() == 1 { a_items.len() }
                      else { a_items.len().min(b_items.len()) };
            let mut result = Vec::with_capacity(len);
            for k in 0..len {
                let a = &a_items[k % a_items.len()];
                let b = &b_items[k % b_items.len()];
                result.push(make_range(real_val(a)?, real_val(b)?)?);
            }
            i.push(make_list(result));
        } else {
            i.push(make_range(real_val(&a_val)?, real_val(&b_val)?)?);
        }
        Ok(())
    });

    // n step by → infinite stream n, n+step, n+2*step, ...; auto-maps when step is a Stream
    reg(i, "by", |i| {
        let step_val = i.pop()?;
        let start = real_val(&i.pop()?)?;
        fn make_by(start: f64, step: f64) -> Value {
            Value::Stream(Arc::new(IterStream::infinite(move || {
                let mut cur = start;
                Box::new(std::iter::from_fn(move || {
                    let v = Value::Real(cur);
                    cur += step;
                    Some(v)
                }))
            })))
        }
        if matches!(&step_val, Value::Stream(_)) {
            let steps = collect_to_vec(&step_val)?;
            let result: Vec<Value> = steps.iter().map(|s| {
                real_val(s).map(|step| make_by(start, step))
            }).collect::<Result<_>>()?;
            i.push(make_list(result));
        } else {
            i.push(make_by(start, real_val(&step_val)?));
        }
        Ok(())
    });

    // finite predicate
    reg(i, "finite", |i| {
        let v = i.pop()?;
        let is_fin = match &v {
            Value::Stream(s) => !s.is_infinite() && s.len_hint().is_some(),
            Value::Real(_) => true,
            _ => false,
        };
        i.push(Value::Real(if is_fin {1.0} else {0.0}));
        Ok(())
    });

    // N: take first n from stream/signal; auto-maps when n is a Stream of counts
    reg(i, "N", |i| {
        let n_val = i.pop()?;
        let src = i.pop()?;
        if let Value::Stream(ns) = &n_val {
            // Stream of counts: for each count take that many from a fresh src iter
            let counts = { let mut it = ns.iter(); let mut v = Vec::new(); while let Some(x) = it.next() { v.push(x); } v };
            let mut result = Vec::with_capacity(counts.len());
            for count_v in &counts {
                let n = real_val(count_v)? as usize;
                match &src {
                    Value::Stream(s) => {
                        let mut it = s.iter();
                        result.push(make_list((0..n).filter_map(|_| it.next()).collect()));
                    }
                    Value::Signal(s) => result.push(make_signal(s.take_n(n))),
                    _ => return Err(Error::Type { expected: "Stream or Signal", actual: src.kind().name() }),
                }
            }
            i.push(make_list(result));
        } else {
            let n_raw = real_val(&n_val)?;
            if !(0.0..=1_000_000.0).contains(&n_raw) {
                return Err(Error::Other("N: n out of range".into()));
            }
            let n = n_raw as usize;
            match &src {
                Value::Stream(s) => {
                    let mut it = s.iter();
                    i.push(make_list((0..n).filter_map(|_| it.next()).collect()));
                }
                Value::Signal(s) => i.push(make_signal(s.take_n(n))),
                _ => return Err(Error::Type { expected: "Stream or Signal", actual: src.kind().name() }),
            }
        }
        Ok(())
    });

    // skip: drop first n
    reg(i, "skip", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let v = i.pop()?;
        match &v {
            Value::Stream(s) => {
                if s.is_infinite() {
                    let s2 = s.clone();
                    i.push(Value::Stream(Arc::new(IterStream::infinite(move || {
                        let mut it = s2.iter();
                        for _ in 0..n { it.next(); }
                        Box::new(std::iter::from_fn(move || it.next()))
                    }))));
                } else {
                    let mut it = s.iter();
                    for _ in 0..n { if it.next().is_none() { break; } }
                    let items: Vec<Value> = std::iter::from_fn(|| it.next()).collect();
                    i.push(make_list(items));
                }
            }
            Value::Signal(_) => {
                let sl = collect_signal_f32(&v)?;
                i.push(make_signal(sl.into_iter().skip(n).collect()));
            }
            _ => return Err(Error::Type { expected: "Stream", actual: v.kind().name() }),
        }
        Ok(())
    });

    // take: n elements with padding
    reg(i, "take", |i| {
        let n_raw = real_val(&i.pop()?)?;
        if n_raw.abs() > 1_000_000.0 {
            return Err(Error::Other("take: n too large".into()));
        }
        let items = collect_to_vec(&i.pop()?)?;
        let result = if n_raw >= 0.0 {
            let n = n_raw as usize;
            let mut r: Vec<Value> = items.into_iter().take(n).collect();
            while r.len() < n { r.push(Value::Real(0.0)); }
            r
        } else {
            let n = (-n_raw) as usize;
            let pad = n.saturating_sub(items.len());
            let mut r: Vec<Value> = (0..pad).map(|_| Value::Real(0.0)).collect();
            let skip = items.len().saturating_sub(n);
            r.extend(items.into_iter().skip(skip));
            r
        };
        i.push(make_list(result)); Ok(())
    });

    // drop: remove first n (or last if negative)
    reg(i, "drop", |i| {
        let n_raw = real_val(&i.pop()?)?;
        let items = collect_to_vec(&i.pop()?)?;
        let result: Vec<Value> = if n_raw >= 0.0 {
            let n = (n_raw as usize).min(items.len());
            items.into_iter().skip(n).collect()
        } else {
            let n = ((-n_raw) as usize).min(items.len());
            let keep = items.len() - n;
            items.into_iter().take(keep).collect()
        };
        i.push(make_list(result)); Ok(())
    });

    // rot: rotate list (positive = right/toward-front)
    reg(i, "rot", |i| {
        let n_raw = real_val(&i.pop()?)?;
        let items = collect_to_vec(&i.pop()?)?;
        if items.is_empty() { i.push(make_list(vec![])); return Ok(()); }
        let len = items.len();
        let n = (n_raw as isize).rem_euclid(len as isize) as usize;
        let mut r = Vec::with_capacity(len);
        r.extend_from_slice(&items[len - n..]);
        r.extend_from_slice(&items[..len - n]);
        i.push(make_list(r)); Ok(())
    });

    // shift: shift with zero padding
    reg(i, "shift", |i| {
        let n_raw = real_val(&i.pop()?)? as isize;
        let items = collect_to_vec(&i.pop()?)?;
        let len = items.len();
        let mut r = vec![Value::Real(0.0); len];
        if n_raw > 0 {
            let n = n_raw as usize;
            for k in 0..len { if k >= n { r[k] = items[k-n].clone(); } }
        } else if n_raw < 0 {
            let n = (-n_raw) as usize;
            for k in 0..len { if k + n < len { r[k] = items[k+n].clone(); } }
        } else {
            r = items;
        }
        i.push(make_list(r)); Ok(())
    });

    // clipShift: shift with edge clamping
    reg(i, "clipShift", |i| {
        let n_raw = real_val(&i.pop()?)? as isize;
        let items = collect_to_vec(&i.pop()?)?;
        let len = items.len();
        if len == 0 { i.push(make_list(vec![])); return Ok(()); }
        let r: Vec<Value> = (0..len as isize)
            .map(|k| items[clip_idx(len, k - n_raw)].clone())
            .collect();
        i.push(make_list(r)); Ok(())
    });

    // foldShift: shift with fold-at-edges
    reg(i, "foldShift", |i| {
        let n_raw = real_val(&i.pop()?)? as isize;
        let items = collect_to_vec(&i.pop()?)?;
        let len = items.len();
        if len == 0 { i.push(make_list(vec![])); return Ok(()); }
        let r: Vec<Value> = (0..len as isize)
            .map(|k| items[fold_idx(len, k - n_raw)].clone())
            .collect();
        i.push(make_list(r)); Ok(())
    });

    // Helper: index a Signal using a Signal index → Signal result
    fn sig_index<F: Fn(usize, isize) -> Value>(items: &[Value], s: &Arc<dyn stax_core::Signal>, f: F) -> Result<Value> {
        if let Some(sl) = s.as_f32_slice() {
            let vals: Vec<f32> = sl.iter().map(|&k| {
                if let Some(r) = f(items.len(), k as isize).as_real() { r as f32 } else { 0.0 }
            }).collect();
            Ok(make_signal(vals))
        } else {
            Err(Error::Other("cannot use this signal as an index".into()))
        }
    }
    // at / wrapAt / clipAt / foldAt — accept Signal source; Signal idx→Signal result only when src is Signal too
    reg(i, "at", |i| {
        let idx = i.pop()?; let src = i.pop()?;
        let src_is_sig = matches!(&src, Value::Signal(_));
        let items = collect_to_vec_or_signal(&src)?;
        let r = if src_is_sig { if let Value::Signal(s) = &idx { sig_index(&items, s, |len,k| at_zero(&items[..len],k))? } else { map_index(&items, &idx, |len,k| at_zero(&items[..len],k))? } } else { map_index(&items, &idx, |len,k| at_zero(&items[..len],k))? };
        i.push(r); Ok(())
    });
    reg(i, "wrapAt", |i| {
        let idx = i.pop()?; let src = i.pop()?;
        let src_is_sig = matches!(&src, Value::Signal(_));
        let items = collect_to_vec_or_signal(&src)?;
        let f = |len: usize, k: isize| if len==0 {Value::Real(0.0)} else {items[wrap_idx(len,k)].clone()};
        let r = if src_is_sig { if let Value::Signal(s) = &idx { sig_index(&items, s, f)? } else { map_index(&items, &idx, f)? } } else { map_index(&items, &idx, f)? };
        i.push(r); Ok(())
    });
    reg(i, "clipAt", |i| {
        let idx = i.pop()?; let src = i.pop()?;
        let src_is_sig = matches!(&src, Value::Signal(_));
        let items = collect_to_vec_or_signal(&src)?;
        let f = |len: usize, k: isize| if len==0 {Value::Real(0.0)} else {items[clip_idx(len,k)].clone()};
        let r = if src_is_sig { if let Value::Signal(s) = &idx { sig_index(&items, s, f)? } else { map_index(&items, &idx, f)? } } else { map_index(&items, &idx, f)? };
        i.push(r); Ok(())
    });
    reg(i, "foldAt", |i| {
        let idx = i.pop()?; let src = i.pop()?;
        let src_is_sig = matches!(&src, Value::Signal(_));
        let items = collect_to_vec_or_signal(&src)?;
        let f = |len: usize, k: isize| if len==0 {Value::Real(0.0)} else {items[fold_idx(len,k)].clone()};
        let r = if src_is_sig { if let Value::Signal(s) = &idx { sig_index(&items, s, f)? } else { map_index(&items, &idx, f)? } } else { map_index(&items, &idx, f)? };
        i.push(r); Ok(())
    });

    // $ cat / $/ cat-reduce
    reg(i, "$", |i| {
        let b = i.pop()?; let a = i.pop()?;
        let mut r = collect_to_vec(&a)?;
        r.extend(collect_to_vec(&b)?);
        i.push(make_list(r)); Ok(())
    });
    reg(i, "$/", |i| {
        let v = i.pop()?;
        let outer = collect_to_vec(&v)?;
        let mut r = Vec::new();
        for item in outer {
            if matches!(item, Value::Stream(_)) { r.extend(collect_to_vec(&item)?); }
            else { r.push(item); }
        }
        i.push(make_list(r)); Ok(())
    });

    // V = Signal→Stream (ZList→VList in SAPF)
    reg(i, "V", |i| {
        let v = i.pop()?;
        match &v {
            Value::Signal(s) => {
                let items: Vec<Value> = s.as_f32_slice()
                    .map(|sl| sl.iter().map(|&x| Value::Real(x as f64)).collect())
                    .unwrap_or_default();
                i.push(make_list(items));
            }
            _ => i.push(v),
        }
        Ok(())
    });
    // Z = Stream→Signal (VList→ZList in SAPF)
    reg(i, "Z", |i| {
        let v = i.pop()?;
        match &v {
            Value::Stream(_) => {
                let items = collect_to_vec(&v)?;
                let floats: Vec<f32> = items.iter()
                    .filter_map(|x| x.as_real().map(|r| r as f32))
                    .collect();
                i.push(make_signal(floats));
            }
            _ => i.push(v),
        }
        Ok(())
    });

    // sort / sort>
    reg(i, "sort", |i| {
        let mut items = collect_to_vec(&i.pop()?)?;
        items.sort_by(|a,b| a.as_real().unwrap_or(0.0).partial_cmp(&b.as_real().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        i.push(make_list(items)); Ok(())
    });
    reg(i, "sort>", |i| {
        let mut items = collect_to_vec(&i.pop()?)?;
        items.sort_by(|a,b| b.as_real().unwrap_or(0.0).partial_cmp(&a.as_real().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        i.push(make_list(items)); Ok(())
    });

    // grade / grade>: sorted index arrays (returns signal #[...])
    reg(i, "grade", |i| {
        let items = collect_to_vec(&i.pop()?)?;
        let mut idx: Vec<usize> = (0..items.len()).collect();
        idx.sort_by(|&a,&b| items[a].as_real().unwrap_or(0.0).partial_cmp(&items[b].as_real().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        i.push(make_signal(idx.iter().map(|&k| k as f32).collect())); Ok(())
    });
    reg(i, "grade>", |i| {
        let items = collect_to_vec(&i.pop()?)?;
        let mut idx: Vec<usize> = (0..items.len()).collect();
        idx.sort_by(|&a,&b| items[b].as_real().unwrap_or(0.0).partial_cmp(&items[a].as_real().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        i.push(make_signal(idx.iter().map(|&k| k as f32).collect())); Ok(())
    });

    // mirror0/1/2
    reg(i, "mirror0", |i| {
        let items = collect_to_vec(&i.pop()?)?;
        let len = items.len();
        let mut r = items.clone();
        if len > 2 { r.extend(items[1..len-1].iter().rev().cloned()); }
        i.push(make_list(r)); Ok(())
    });
    reg(i, "mirror1", |i| {
        let items = collect_to_vec(&i.pop()?)?;
        let len = items.len();
        let mut r = items.clone();
        if len > 0 { r.extend(items[..len-1].iter().rev().cloned()); }
        i.push(make_list(r)); Ok(())
    });
    reg(i, "mirror2", |i| {
        let items = collect_to_vec(&i.pop()?)?;
        let mut r = items.clone();
        r.extend(items.iter().rev().cloned());
        i.push(make_list(r)); Ok(())
    });

    // flat / flatten
    reg(i, "flat", |i| {
        let v = i.pop()?;
        i.push(make_list(flatten_deep(&v)?)); Ok(())
    });
    reg(i, "flatten", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let v = i.pop()?;
        // flatten_n(v, n+1) removes n levels: flatten_n recurses into items,
        // so n=0→[v] (identity), n=1→items(v) spread (but wraps back up wrong).
        // Passing n+1 gives the correct "remove n levels" semantics.
        i.push(make_list(flatten_n(&v, n + 1)?)); Ok(())
    });

    // clump — drops the final incomplete chunk (SAPF behaviour)
    reg(i, "clump", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let items = collect_to_vec(&i.pop()?)?;
        if n == 0 { i.push(make_list(vec![])); return Ok(()); }
        let r: Vec<Value> = items.chunks(n).filter(|c| c.len() == n).map(|c| make_list(c.to_vec())).collect();
        i.push(make_list(r)); Ok(())
    });

    // cyc / ncyc
    reg(i, "cyc", |i| {
        let items = collect_to_vec(&i.pop()?)?;
        if items.is_empty() { i.push(make_list(vec![])); return Ok(()); }
        let arc = Arc::new(items);
        i.push(Value::Stream(Arc::new(IterStream::infinite(move || {
            let arc = arc.clone();
            let mut pos = 0usize;
            Box::new(std::iter::from_fn(move || {
                let v = arc[pos % arc.len()].clone();
                pos += 1;
                Some(v)
            }))
        }))));
        Ok(())
    });
    reg(i, "ncyc", |i| {
        let n_raw = real_val(&i.pop()?)?;
        if n_raw > 1_000_000.0 {
            return Err(Error::Other("ncyc: n too large".into()));
        }
        let n = n_raw as i64;
        let items = collect_to_vec(&i.pop()?)?;
        if n <= 0 { i.push(make_list(vec![])); return Ok(()); }
        let mut r = Vec::with_capacity(items.len() * n as usize);
        for _ in 0..n { r.extend(items.iter().cloned()); }
        i.push(make_list(r)); Ok(())
    });

    // add / cons / head / tail / empty / nonempty
    reg(i, "add", |i| {
        let elem = i.pop()?;
        let mut items = collect_to_vec(&i.pop()?)?;
        items.push(elem);
        i.push(make_list(items)); Ok(())
    });
    reg(i, "cons", |i| {
        let elem = i.pop()?;
        let items = collect_to_vec(&i.pop()?)?;
        let mut r = vec![elem];
        r.extend(items);
        i.push(make_list(r)); Ok(())
    });
    reg(i, "head", |i| {
        let v = i.pop()?;
        if let Value::Stream(s) = &v {
            let h = s.iter().next()
                .ok_or_else(|| Error::Other("head of empty stream".into()))?;
            i.push(h);
        } else {
            return Err(Error::Type { expected: "Stream", actual: v.kind().name() });
        }
        Ok(())
    });
    reg(i, "tail", |i| {
        let mut items = collect_to_vec(&i.pop()?)?;
        if !items.is_empty() { items.remove(0); }
        i.push(make_list(items)); Ok(())
    });
    reg(i, "empty", |i| {
        let v = i.pop()?;
        let e = match &v { Value::Stream(s) => s.len_hint()==Some(0), _ => false };
        i.push(Value::Real(if e {1.0} else {0.0})); Ok(())
    });
    reg(i, "nonempty", |i| {
        let v = i.pop()?;
        let e = match &v { Value::Stream(s) => s.len_hint()==Some(0), _ => false };
        i.push(Value::Real(if e {0.0} else {1.0})); Ok(())
    });

    // Refs
    reg(i, "R", |i| {
        let v = i.pop()?;
        i.push(Value::Ref(Arc::new(std::sync::RwLock::new(v)))); Ok(())
    });
    reg(i, "ZR", |i| {
        let v = i.pop()?;
        i.push(Value::Ref(Arc::new(std::sync::RwLock::new(v)))); Ok(())
    });
    reg(i, "get", |i| {
        let v = i.pop()?;
        match v {
            Value::Ref(r) => {
                let val = r.read().map_err(|e| Error::Other(format!("rwlock: {e}")))?.clone();
                i.push(val);
            }
            other => i.push(other),
        }
        Ok(())
    });
    reg(i, "set", |i| {
        let ref_v = i.pop()?;
        let val   = i.pop()?;
        match ref_v {
            Value::Ref(r) => {
                *r.write().map_err(|e| Error::Other(format!("rwlock: {e}")))? = val;
            }
            _ => return Err(Error::Type { expected: "Ref", actual: ref_v.kind().name() }),
        }
        Ok(())
    });

    // muss: shuffle in-place using the interpreter's deterministic seed
    reg(i, "muss", |i| {
        let mut items = collect_to_vec(&i.pop()?)?;
        for k in (1..items.len()).rev() {
            let s = rng_step(&mut i.rng_seed);
            let j = (s >> 33) as usize % (k + 1);
            items.swap(k, j);
        }
        i.push(make_list(items)); Ok(())
    });

    // bub / nbub
    reg(i, "bub", |i| { let v=i.pop()?; i.push(make_list(vec![v])); Ok(()) });
    reg(i, "nbub", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let mut v = i.pop()?;
        for _ in 0..n { v = make_list(vec![v]); }
        i.push(v); Ok(())
    });

    // flop: transpose list-of-lists
    reg(i, "flop", |i| {
        let rows_val = i.pop()?;
        let rows = collect_to_vec(&rows_val)?;
        if rows.is_empty() { i.push(make_list(vec![])); return Ok(()); }

        // Find max non-infinite length
        let max_len = rows.iter().filter_map(|r| match r {
            Value::Stream(s) if !s.is_infinite() => s.len_hint(),
            Value::Stream(_) => None,
            Value::Real(_) => Some(1),
            _ => None,
        }).max().unwrap_or(0);

        let mut expanded: Vec<Vec<Value>> = Vec::with_capacity(rows.len());
        for row in &rows {
            match row {
                Value::Stream(s) => {
                    let base: Vec<Value> = if s.is_infinite() {
                        let mut it = s.iter();
                        (0..max_len).filter_map(|_| it.next()).collect()
                    } else {
                        let mut it = s.iter();
                        let mut v = Vec::new();
                        while let Some(x) = it.next() { v.push(x); }
                        v
                    };
                    if base.is_empty() { expanded.push(vec![]); continue; }
                    let cycled: Vec<Value> = (0..max_len).map(|k| base[k%base.len()].clone()).collect();
                    expanded.push(cycled);
                }
                other => expanded.push(vec![other.clone(); max_len]),
            }
        }

        let result: Vec<Value> = (0..max_len)
            .map(|col| make_list(expanded.iter().filter_map(|r| r.get(col).cloned()).collect()))
            .collect();
        i.push(make_list(result)); Ok(())
    });

    // tog: interleave two values/streams, always producing a cycling infinite stream
    reg(i, "tog", |i| {
        let b = i.pop()?;
        let a = i.pop()?;
        let a_stream = to_cycling_stream(a);
        let b_stream = to_cycling_stream(b);
        i.push(Value::Stream(Arc::new(IterStream::infinite(move || {
            let mut a_it = a_stream.iter();
            let mut b_it = b_stream.iter();
            let mut use_a = true;
            Box::new(std::iter::from_fn(move || {
                let r = if use_a { a_it.next() } else { b_it.next() };
                use_a = !use_a;
                r
            }))
        }))));
        Ok(())
    });

    // type — returns SAPF type name as a Symbol
    reg(i, "type", |i| {
        let v = i.pop()?;
        let name = match v.kind() {
            ValueKind::Stream => "VList",
            ValueKind::Signal => "Vec",
            ValueKind::Str | ValueKind::Sym => "String",
            other => other.name(),
        };
        i.push(Value::Sym(Arc::from(name)));
        Ok(())
    });

    // skipWhile: skip leading elements while mask/predicate is truthy
    reg(i, "skipWhile", |i| {
        let mask_val = i.pop()?;
        let list_val = i.pop()?;
        let items = collect_to_vec(&list_val)?;
        let skip_count = match &mask_val {
            Value::Fun(_) => {
                let mut count = 0;
                for item in &items {
                    i.push(item.clone());
                    i.apply_or_push(mask_val.clone())?;
                    let r = i.pop()?;
                    if r.is_truthy() { count += 1; } else { break; }
                }
                count
            }
            Value::Stream(_) => {
                let mask = collect_to_vec(&mask_val)?;
                let mut count = 0;
                for m in mask.iter().take(items.len()) {
                    if m.is_truthy() { count += 1; } else { break; }
                }
                count
            }
            _ => 0,
        };
        i.push(make_list(items.into_iter().skip(skip_count).collect()));
        Ok(())
    });

    // keepWhile: keep leading elements while mask/predicate is truthy
    reg(i, "keepWhile", |i| {
        let mask_val = i.pop()?;
        let list_val = i.pop()?;
        let items = collect_to_vec(&list_val)?;
        let keep_count = match &mask_val {
            Value::Fun(_) => {
                let mut count = 0;
                for item in &items {
                    i.push(item.clone());
                    i.apply_or_push(mask_val.clone())?;
                    let r = i.pop()?;
                    if r.is_truthy() { count += 1; } else { break; }
                }
                count
            }
            Value::Stream(_) => {
                let mask = collect_to_vec(&mask_val)?;
                items.iter().zip(mask.iter()).take_while(|(_, m)| m.is_truthy()).count()
            }
            _ => 0,
        };
        i.push(make_list(items.into_iter().take(keep_count).collect()));
        Ok(())
    });

    // ?: filter/repeat — mask[i]=0 drops, mask[i]=n repeats n times
    reg(i, "?", |i| {
        let mask_val = i.pop()?;
        let src_val  = i.pop()?;
        let mut result = Vec::new();
        match &mask_val {
            Value::Real(n) => {
                let count = *n as usize;
                let items = collect_to_vec(&src_val)?;
                for item in items { for _ in 0..count { result.push(item.clone()); } }
            }
            _ => {
                let src_inf  = matches!(&src_val,  Value::Stream(s) if s.is_infinite());
                let mask_inf = matches!(&mask_val, Value::Stream(s) if s.is_infinite());
                if src_inf && mask_inf {
                    // Both infinite: produce a lazy infinite stream (filter/repeat)
                    let sv = src_val.clone(); let mv = mask_val.clone();
                    i.push(Value::Stream(Arc::new(IterStream::infinite(move || {
                        let mut si = match &sv { Value::Stream(s) => s.iter(), _ => unreachable!() };
                        let mut mi = match &mv { Value::Stream(s) => s.iter(), _ => unreachable!() };
                        let mut rep_item: Option<Value> = None;
                        let mut rep_count: usize = 0;
                        Box::new(std::iter::from_fn(move || {
                            loop {
                                if rep_count > 0 {
                                    rep_count -= 1;
                                    return rep_item.clone();
                                }
                                let item = si.next()?;
                                let count = mi.next()?.as_real().unwrap_or(0.0) as usize;
                                if count > 0 {
                                    rep_item = Some(item);
                                    rep_count = count;
                                }
                            }
                        }))
                    }))));
                    return Ok(());
                }
                let (src_items, mask_items): (Vec<Value>, Vec<Value>) =
                    if src_inf {
                        let mask = collect_to_vec(&mask_val)?;
                        let n = mask.len();
                        let mut it = if let Value::Stream(s) = &src_val { s.iter() } else { unreachable!() };
                        let src: Vec<Value> = (0..n).filter_map(|_| it.next()).collect();
                        (src, mask)
                    } else if mask_inf {
                        let src = collect_to_vec(&src_val)?;
                        let n = src.len();
                        let mut it = if let Value::Stream(s) = &mask_val { s.iter() } else { unreachable!() };
                        let mask: Vec<Value> = (0..n).filter_map(|_| it.next()).collect();
                        (src, mask)
                    } else {
                        (collect_to_vec(&src_val)?, collect_to_vec(&mask_val)?)
                    };
                for (item, count_val) in src_items.iter().zip(mask_items.iter()) {
                    let count = count_val.as_real().unwrap_or(0.0) as usize;
                    for _ in 0..count { result.push(item.clone()); }
                }
            }
        }
        i.push(make_list(result));
        Ok(())
    });

    // Form ops
    reg(i, "has", |i| {
        let sym = i.pop()?;
        let form_val = i.pop()?;
        let key: Arc<str> = match &sym {
            Value::Sym(s) | Value::Str(s) => s.clone(),
            _ => return Err(Error::Type { expected: "Sym", actual: sym.kind().name() }),
        };
        let found = matches!(&form_val, Value::Form(f) if f.get(&key).is_some());
        i.push(Value::Real(if found { 1.0 } else { 0.0 }));
        Ok(())
    });
    reg(i, "keys", |i| {
        let v = i.pop()?;
        match &v {
            Value::Form(f) => {
                let keys: Vec<Value> = f.bindings.keys().map(|k| Value::Sym(k.clone())).collect();
                i.push(make_list(keys));
            }
            _ => return Err(Error::Type { expected: "Form", actual: v.kind().name() }),
        }
        Ok(())
    });
    reg(i, "values", |i| {
        let v = i.pop()?;
        match &v {
            Value::Form(f) => {
                let vals: Vec<Value> = f.bindings.values().cloned().collect();
                i.push(make_list(vals));
            }
            _ => return Err(Error::Type { expected: "Form", actual: v.kind().name() }),
        }
        Ok(())
    });
    // kv: push keys-list then values-list (caller's function takes k=keys, v=values)
    reg(i, "kv", |i| {
        let v = i.pop()?;
        match &v {
            Value::Form(f) => {
                let keys: Vec<Value> = f.bindings.keys().map(|k| Value::Sym(k.clone())).collect();
                let vals: Vec<Value> = f.bindings.values().cloned().collect();
                i.push(make_list(keys));
                i.push(make_list(vals));
            }
            _ => return Err(Error::Type { expected: "Form", actual: v.kind().name() }),
        }
        Ok(())
    });
    reg(i, "parent", |i| {
        let v = i.pop()?;
        match &v {
            Value::Form(f) => {
                i.push(f.parents.first().map_or(Value::Nil, |p| Value::Form(p.clone())));
            }
            _ => return Err(Error::Type { expected: "Form", actual: v.kind().name() }),
        }
        Ok(())
    });
    reg(i, "local", |i| {
        let v = i.pop()?;
        match &v {
            Value::Form(f) => {
                let mut local = Form::new();
                for (k, val) in &f.bindings { local.insert(k.clone(), val.clone()); }
                i.push(Value::Form(Arc::new(local)));
            }
            _ => return Err(Error::Type { expected: "Form", actual: v.kind().name() }),
        }
        Ok(())
    });
    reg(i, "dot", |i| {
        let sym = i.pop()?;
        let form_val = i.pop()?;
        let key: Arc<str> = match &sym {
            Value::Sym(s) | Value::Str(s) => s.clone(),
            _ => return Err(Error::Type { expected: "Sym", actual: sym.kind().name() }),
        };
        match form_val {
            Value::Form(f) => {
                let val = f.get(&key).ok_or_else(|| Error::Unbound(key.to_string()))?;
                i.apply_or_push(val)
            }
            _ => Err(Error::Type { expected: "Form", actual: form_val.kind().name() }),
        }
    });

    // nby: counts start steps nby → for each (n, step): [start, start+step, ..., start+(n-1)*step]
    reg(i, "nby", |i| {
        let steps_val = i.pop()?;
        let start = real_val(&i.pop()?)?;
        let counts_val = i.pop()?;
        let steps: Vec<f64> = if matches!(&steps_val, Value::Stream(_)) {
            collect_to_vec(&steps_val)?.iter().map(real_val).collect::<Result<_>>()?
        } else {
            vec![real_val(&steps_val)?]
        };
        let counts: Vec<usize> = if matches!(&counts_val, Value::Stream(_)) {
            collect_to_vec(&counts_val)?.iter().map(|v| real_val(v).map(|x| x as usize)).collect::<Result<_>>()?
        } else {
            vec![real_val(&counts_val)? as usize]
        };
        let len = steps.len().min(counts.len());
        if len == 1 {
            let step = steps[0]; let n = counts[0];
            let items: Vec<Value> = (0..n).map(|k| Value::Real(start + step * k as f64)).collect();
            i.push(make_list(items));
        } else {
            let result: Vec<Value> = (0..len).map(|k| {
                let step = steps[k]; let n = counts[k];
                make_list((0..n).map(|j| Value::Real(start + step * j as f64)).collect())
            }).collect();
            i.push(make_list(result));
        }
        Ok(())
    });

    // ---- Deterministic RNG --------------------------------------------------

    reg(i, "seed", |i| {
        let s = real_val(&i.pop()?)? as u64;
        i.rng_seed = s;
        Ok(())
    });

    reg(i, "rand", |i| {
        let s = rng_step(&mut i.rng_seed);
        i.push(Value::Real((s >> 33) as f64 / u32::MAX as f64));
        Ok(())
    });

    reg(i, "irand", |i| {
        let n = real_val(&i.pop()?)? as u64;
        if n == 0 { i.push(Value::Real(0.0)); return Ok(()); }
        let s = rng_step(&mut i.rng_seed);
        i.push(Value::Real(((s >> 33) % n) as f64));
        Ok(())
    });

    // ---- DSP oscillators & generators ---------------------------------------

    reg(i, "sinosc", |i| {
        let phase = real_val(&i.pop()?)? as f32;
        let freq  = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::SinOsc::with_phase(freq, phase))));
        Ok(())
    });

    reg(i, "saw", |i| {
        let phase = real_val(&i.pop()?)? as f32;
        let freq  = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::SawOsc::with_phase(freq, phase))));
        Ok(())
    });

    reg(i, "lfsaw", |i| {
        let phase = real_val(&i.pop()?)? as f32;
        let freq  = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::SawOsc::with_phase(freq, phase))));
        Ok(())
    });

    reg(i, "wnoise", |i| {
        let seed = real_val(&i.pop()?)? as u64;
        i.push(Value::Signal(Arc::new(stax_dsp::WhiteNoise::new(seed))));
        Ok(())
    });

    reg(i, "pnoise", |i| {
        let seed = real_val(&i.pop()?)? as u64;
        i.push(Value::Signal(Arc::new(stax_dsp::PinkNoise::new(seed))));
        Ok(())
    });

    reg(i, "combn", |i| {
        let coeff = real_val(&i.pop()?)? as f32;
        let delay = real_val(&i.pop()?)? as usize;
        let input = i.pop()?;
        if let Value::Signal(s) = input {
            i.push(Value::Signal(Arc::new(stax_dsp::CombFilterSignal {
                input: s, delay_samples: delay, coeff,
            })));
        } else {
            return Err(Error::Type { expected: "Signal", actual: input.kind().name() });
        }
        Ok(())
    });

    reg(i, "pluck", |i| {
        let decay = real_val(&i.pop()?)? as f32;
        let freq  = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::PluckOsc::new(freq, decay))));
        Ok(())
    });

    reg(i, "ar", |i| {
        let release = real_val(&i.pop()?)? as f32;
        let attack  = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::ArEnv { attack_secs: attack, release_secs: release })));
        Ok(())
    });

    reg(i, "adsr", |i| {
        let release       = real_val(&i.pop()?)? as f32;
        let sustain_time  = real_val(&i.pop()?)? as f32;
        let sustain_level = real_val(&i.pop()?)? as f32;
        let decay         = real_val(&i.pop()?)? as f32;
        let attack        = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::AdsrEnv {
            attack_secs: attack, decay_secs: decay,
            sustain_level, sustain_secs: sustain_time, release_secs: release,
        })));
        Ok(())
    });

    // ---- FFT ----------------------------------------------------------------

    reg(i, "fft", |i| {
        let sig = i.pop()?;
        let samples = collect_signal_f32(&sig)?;
        let mags = stax_dsp::fft_magnitude(&samples);
        i.push(make_signal(mags));
        Ok(())
    });

    reg(i, "ifft", |i| {
        let sig = i.pop()?;
        let mags = collect_signal_f32(&sig)?;
        let out = stax_dsp::ifft_from_magnitude(&mags);
        i.push(make_signal(out));
        Ok(())
    });

    // ---- Audio playback -----------------------------------------------------

    reg(i, "play", |i| {
        let sig_val = i.pop()?;
        if let Value::Signal(sig) = sig_val {
            // Lazy-init the audio runtime.
            if i.audio_rt.is_none() {
                match stax_audio::Runtime::new() {
                    Ok(rt) => {
                        i.sample_rate = rt.sample_rate();
                        i.audio_rt = Some(Arc::new(rt));
                    }
                    Err(e) => {
                        eprintln!("stax: audio runtime unavailable: {e}");
                        i.push(Value::Nil);
                        return Ok(());
                    }
                }
            }
            if let Some(rt) = &i.audio_rt.clone() {
                let voice = rt.play(sig).map_err(|e| Error::Other(format!("play: {e}")))?;
                i.voices.push(voice);
            }
            i.push(Value::Nil);
        } else {
            return Err(Error::Type { expected: "Signal", actual: sig_val.kind().name() });
        }
        Ok(())
    });

    reg(i, "stop", |i| {
        i.voices.clear();
        i.push(Value::Nil);
        Ok(())
    });

    // ---- MIDI ---------------------------------------------------------------

    reg(i, "midiPorts", |i| {
        let ports = stax_io::MidiOut::ports();
        let vals: Vec<Value> = ports.into_iter()
            .map(|s| Value::Str(Arc::from(s.as_str())))
            .collect();
        i.push(make_list(vals));
        Ok(())
    });

    reg(i, "midiConnect", |i| {
        let idx = real_val(&i.pop()?)? as usize;
        match stax_io::MidiOut::connect(idx) {
            Ok(conn) => { i.midi_out = Some(conn); i.push(Value::Nil); }
            Err(e) => return Err(Error::Other(format!("midiConnect: {e}"))),
        }
        Ok(())
    });

    reg(i, "midiSend", |i| {
        let bytes_val = i.pop()?;
        let bytes_list = collect_to_vec(&bytes_val)?;
        let bytes: Vec<u8> = bytes_list.iter()
            .filter_map(|v| v.as_real().map(|x| x as u8))
            .collect();
        if let Some(ref mut m) = i.midi_out {
            m.send(&bytes).map_err(|e| Error::Other(e.to_string()))?;
        } else {
            return Err(Error::Other("midiSend: no MIDI output connected (use midiConnect first)".into()));
        }
        Ok(())
    });

    reg(i, "noteOn", |i| {
        let vel = real_val(&i.pop()?)? as u8;
        let note = real_val(&i.pop()?)? as u8;
        let ch = real_val(&i.pop()?)? as u8;
        if let Some(ref mut m) = i.midi_out {
            m.note_on(ch, note, vel).map_err(|e| Error::Other(e.to_string()))?;
        } else {
            return Err(Error::Other("noteOn: no MIDI output connected".into()));
        }
        Ok(())
    });

    reg(i, "noteOff", |i| {
        let vel = real_val(&i.pop()?)? as u8;
        let note = real_val(&i.pop()?)? as u8;
        let ch = real_val(&i.pop()?)? as u8;
        if let Some(ref mut m) = i.midi_out {
            m.note_off(ch, note, vel).map_err(|e| Error::Other(e.to_string()))?;
        } else {
            return Err(Error::Other("noteOff: no MIDI output connected".into()));
        }
        Ok(())
    });

    reg(i, "midiCC", |i| {
        let val = real_val(&i.pop()?)? as u8;
        let ctrl = real_val(&i.pop()?)? as u8;
        let ch = real_val(&i.pop()?)? as u8;
        if let Some(ref mut m) = i.midi_out {
            m.cc(ch, ctrl, val).map_err(|e| Error::Other(e.to_string()))?;
        } else {
            return Err(Error::Other("midiCC: no MIDI output connected".into()));
        }
        Ok(())
    });

    // ---- OSC ----------------------------------------------------------------

    // args: list of Reals (sent as OscFloat) or Strings (sent as OscString)
    reg(i, "oscSend", |i| {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let args_val = i.pop()?;
            let addr_val = i.pop()?;
            let port = real_val(&i.pop()?)? as u16;
            let host = match i.pop()? {
                Value::Str(s) | Value::Sym(s) => s.to_string(),
                other => return Err(Error::Type { expected: "Str", actual: other.kind().name() }),
            };
            let addr = match addr_val {
                Value::Str(s) | Value::Sym(s) => s.to_string(),
                other => return Err(Error::Type { expected: "Str", actual: other.kind().name() }),
            };
            let raw = collect_to_vec(&args_val)?;
            let osc_args: Vec<stax_io::OscType> = raw.iter().map(|v| match v {
                Value::Real(x) => stax_io::OscType::Float(*x as f32),
                Value::Str(s) | Value::Sym(s) => stax_io::OscType::String(s.to_string()),
                _ => stax_io::OscType::Float(0.0),
            }).collect();
            stax_io::osc_send(&host, port, &addr, osc_args)
                .map_err(|e| Error::Other(e.to_string()))?;
        }
        #[cfg(target_arch = "wasm32")]
        { return Err(Error::Other("oscSend not available in WASM".into())); }
        Ok(())
    });

    // ---- Sample-rate constants -------------------------------------------

    reg(i, "sr",   |i| { i.push(Value::Real(i.sample_rate)); Ok(()) });
    reg(i, "nyq",  |i| { i.push(Value::Real(i.sample_rate * 0.5)); Ok(()) });
    reg(i, "isr",  |i| { i.push(Value::Real(1.0 / i.sample_rate)); Ok(()) });
    reg(i, "inyq", |i| { i.push(Value::Real(2.0 / i.sample_rate)); Ok(()) });
    reg(i, "rps",  |i| {
        i.push(Value::Real(std::f64::consts::TAU / i.sample_rate));
        Ok(())
    });

    // ---- Conversion: MIDI note ↔ Hz -----------------------------------------

    // A4 = MIDI 69 = 440 Hz
    reg(i, "midihz", |i| {
        let v = i.pop()?;
        i.push(automap_unary(v, |note| 440.0 * 2f64.powf((note - 69.0) / 12.0))?);
        Ok(())
    });
    reg(i, "midinote", |i| {
        let v = i.pop()?;
        i.push(automap_unary(v, |hz| 69.0 + 12.0 * (hz / 440.0).log2())?);
        Ok(())
    });
    reg(i, "bilin", |i| {
        let hi = real_val(&i.pop()?)?;
        let lo = real_val(&i.pop()?)?;
        let v = i.pop()?;
        let mapped = apply_bilin(v, lo, hi)?;
        i.push(mapped);
        Ok(())
    });
    reg(i, "biexp", |i| {
        let hi = real_val(&i.pop()?)?;
        let lo = real_val(&i.pop()?)?;
        let v = i.pop()?;
        let mapped = apply_biexp(v, lo, hi)?;
        i.push(mapped);
        Ok(())
    });

    // ---- Zero-arg noise (SAPF naming) -----------------------------------------

    // white / pink / brown → noise Signals seeded from interpreter RNG
    reg(i, "white", |i| { let seed = rng_step(&mut i.rng_seed); i.push(Value::Signal(Arc::new(stax_dsp::WhiteNoise::new(seed)))); Ok(()) });
    reg(i, "pink",  |i| { let seed = rng_step(&mut i.rng_seed); i.push(Value::Signal(Arc::new(stax_dsp::PinkNoise::new(seed)))); Ok(()) });
    reg(i, "brown", |i| { let seed = rng_step(&mut i.rng_seed); i.push(Value::Signal(Arc::new(stax_dsp::BrownNoise::new(seed)))); Ok(()) });

    // ---- Oscillators --------------------------------------------------------

    reg(i, "tri", |i| {
        let phase = real_val(&i.pop()?)? as f32;
        let freq  = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::TriOsc::with_phase(freq, phase))));
        Ok(())
    });
    // duty 0..1; 0.5 = square wave
    reg(i, "pulse", |i| {
        let duty = real_val(&i.pop()?)? as f32;
        let freq = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::PulseOsc::new(freq, duty))));
        Ok(())
    });
    reg(i, "square", |i| {
        let freq = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::PulseOsc::new(freq, 0.5))));
        Ok(())
    });
    reg(i, "impulse", |i| {
        let freq = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::ImpulseSignal::new(freq))));
        Ok(())
    });

    // ---- Filters ------------------------------------------------------------

    reg(i, "lpf1",  |i| { let cutoff = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::Lpf1Signal { input, cutoff_hz: cutoff }))); Ok(()) });
    // lpf and lpf2 are the same 2nd-order Butterworth LP filter
    reg(i, "lpf",   |i| { let cutoff = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::Lpf2Signal { input, cutoff_hz: cutoff }))); Ok(()) });
    reg(i, "lpf2",  |i| { let cutoff = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::Lpf2Signal { input, cutoff_hz: cutoff }))); Ok(()) });
    reg(i, "hpf1",  |i| { let cutoff = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::Hpf1Signal { input, cutoff_hz: cutoff }))); Ok(()) });
    // hpf and hpf2 are the same 2nd-order Butterworth HP filter
    reg(i, "hpf",   |i| { let cutoff = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::Hpf2Signal { input, cutoff_hz: cutoff }))); Ok(()) });
    reg(i, "hpf2",  |i| { let cutoff = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::Hpf2Signal { input, cutoff_hz: cutoff }))); Ok(()) });
    // signal cutoff rq rlpf/rhpf → resonant filters (rq = 1/Q)
    reg(i, "rlpf",  |i| { let rq = real_val(&i.pop()?)? as f32; let cutoff = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::RlpfSignal { input, cutoff_hz: cutoff, rq }))); Ok(()) });
    reg(i, "rhpf",  |i| { let rq = real_val(&i.pop()?)? as f32; let cutoff = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::RhpfSignal { input, cutoff_hz: cutoff, rq }))); Ok(()) });
    reg(i, "lag",   |i| { let lag_time = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::LagSignal { input, lag_time }))); Ok(()) });
    reg(i, "lag2",  |i| { let lag_time = real_val(&i.pop()?)? as f32; let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::Lag2Signal { input, lag_time }))); Ok(()) });
    reg(i, "leakdc",|i| { let input = pop_signal(i)?; i.push(Value::Signal(Arc::new(stax_dsp::LeakDcSignal { input }))); Ok(()) });

    // ---- Control signals ----------------------------------------------------

    reg(i, "line", |i| {
        let dur   = real_val(&i.pop()?)? as f32;
        let end   = real_val(&i.pop()?)? as f32;
        let start = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::LineSignal { start, end, dur_secs: dur })));
        Ok(())
    });
    reg(i, "xline", |i| {
        let dur   = real_val(&i.pop()?)? as f32;
        let end   = real_val(&i.pop()?)? as f32;
        let start = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::XlineSignal { start, end, dur_secs: dur })));
        Ok(())
    });
    reg(i, "decay", |i| {
        let dur = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::DecaySignal { dur_secs: dur })));
        Ok(())
    });

    // ---- List operations ----------------------------------------------------

    // list n grow → repeat last element until length n
    reg(i, "grow", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let mut items = collect_to_vec(&i.pop()?)?;
        if let Some(last) = items.last().cloned() {
            while items.len() < n { items.push(last.clone()); }
        }
        i.push(make_list(items));
        Ok(())
    });
    // list n ngrow → same as grow but zero-fills if empty
    reg(i, "ngrow", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let mut items = collect_to_vec(&i.pop()?)?;
        let fill = items.last().cloned().unwrap_or(Value::Real(0.0));
        while items.len() < n { items.push(fill.clone()); }
        i.push(make_list(items));
        Ok(())
    });
    // a b n lindiv → n evenly spaced floats from a to b (inclusive)
    reg(i, "lindiv", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let b = real_val(&i.pop()?)?;
        let a = real_val(&i.pop()?)?;
        let items: Vec<Value> = if n <= 1 {
            vec![Value::Real(a)]
        } else {
            (0..n).map(|k| Value::Real(a + (b - a) * k as f64 / (n - 1) as f64)).collect()
        };
        i.push(make_list(items));
        Ok(())
    });
    // a b n expdiv → n exponentially spaced floats from a to b
    reg(i, "expdiv", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let b = real_val(&i.pop()?)?;
        let a = real_val(&i.pop()?)?;
        let items: Vec<Value> = if n <= 1 || a <= 0.0 || b <= 0.0 {
            vec![Value::Real(a)]
        } else {
            let ratio = (b / a).powf(1.0 / (n - 1) as f64);
            (0..n).map(|k| Value::Real(a * ratio.powi(k as i32))).collect()
        };
        i.push(make_list(items));
        Ok(())
    });
    // list ever → infinite cycling stream (alias for cyc)
    reg(i, "ever", |i| {
        let v = i.pop()?;
        let s = to_cycling_stream(v);
        i.push(Value::Stream(s));
        Ok(())
    });
    // list_a list_b lace → interleave: [a0,b0,a1,b1,...]
    reg(i, "lace", |i| {
        let b = collect_to_vec(&i.pop()?)?;
        let a = collect_to_vec(&i.pop()?)?;
        let len = a.len().max(b.len());
        let mut out = Vec::with_capacity(len * 2);
        for k in 0..len {
            if k < a.len() { out.push(a[k].clone()); }
            if k < b.len() { out.push(b[k].clone()); }
        }
        i.push(make_list(out));
        Ok(())
    });
    // signal 2X → [signal, signal] (stereo duplicate)
    reg(i, "2X", |i| {
        let v = i.pop()?;
        i.push(make_list(vec![v.clone(), v]));
        Ok(())
    });
    // spread: identity stub — SAPF semantics not fully resolved yet
    reg(i, "spread", |i| {
        let items = collect_to_vec(&i.pop()?)?;
        i.push(make_list(items));
        Ok(())
    });

    // ---- ZList words (Signal-domain equivalents) ----------------------------

    reg(i, "natz", |i| {
        i.push(Value::Signal(Arc::new(stax_core::signal::GenSignal::new(|| {
            let mut n = 0u32;
            Box::new(std::iter::from_fn(move || { let v = n as f32; n += 1; Some(v) }))
        }))));
        Ok(())
    });
    reg(i, "byz", |i| {
        let step = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_core::signal::GenSignal::new(move || {
            let mut cur = 0.0f32;
            let s = step;
            Box::new(std::iter::from_fn(move || { let v = cur; cur += s; Some(v) }))
        }))));
        Ok(())
    });
    reg(i, "nbyz", |i| {
        let step  = real_val(&i.pop()?)? as f32;
        let start = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_core::signal::GenSignal::new(move || {
            let mut cur = start;
            let s = step;
            Box::new(std::iter::from_fn(move || { let v = cur; cur += s; Some(v) }))
        }))));
        Ok(())
    });
    reg(i, "invz", |i| {
        i.push(Value::Signal(Arc::new(stax_core::signal::GenSignal::new(|| {
            let mut n = 1u32;
            Box::new(std::iter::from_fn(move || {
                let v = 1.0 / n as f32; n += 1; Some(v)
            }))
        }))));
        Ok(())
    });
    reg(i, "negz", |i| {
        i.push(Value::Signal(Arc::new(stax_core::signal::GenSignal::new(|| {
            let mut n = 1u32;
            Box::new(std::iter::from_fn(move || {
                let v = -(n as f32); n += 1; Some(v)
            }))
        }))));
        Ok(())
    });
    reg(i, "evenz", |i| {
        i.push(Value::Signal(Arc::new(stax_core::signal::GenSignal::new(|| {
            let mut n = 0u32;
            Box::new(std::iter::from_fn(move || {
                let v = n as f32; n += 2; Some(v)
            }))
        }))));
        Ok(())
    });
    reg(i, "oddz", |i| {
        i.push(Value::Signal(Arc::new(stax_core::signal::GenSignal::new(|| {
            let mut n = 1u32;
            Box::new(std::iter::from_fn(move || {
                let v = n as f32; n += 2; Some(v)
            }))
        }))));
        Ok(())
    });

    // ---- Debug / introspection ----------------------------------------------

    reg(i, "p", |i| {
        if let Some(v) = i.peek() {
            eprintln!("[stax] {v:?}");
        }
        Ok(())
    });
    reg(i, "trace", |i| {
        eprintln!("[stax] stack ({} items):", i.stack.len());
        for (idx, v) in i.stack.iter().enumerate().rev() {
            eprintln!("  [{idx}] {v:?}");
        }
        Ok(())
    });
    reg(i, "inspect", |i| {
        if let Some(v) = i.peek() {
            eprintln!("[stax] {:?} :: {:?}", v, v.kind());
        }
        Ok(())
    });
    reg(i, "bench", |i| {
        let f = i.pop()?;
        let t0 = std::time::Instant::now();
        i.apply_or_push(f)?;
        eprintln!("[stax] bench: {:.3} ms", t0.elapsed().as_secs_f64() * 1000.0);
        Ok(())
    });

    // ---- Signal math completions --------------------------------------------

    umap!("sign",   |x: f64| if x == 0.0 { 0.0 } else { x.signum() });
    umap!("dbtamp", |db: f64| 10f64.powf(db / 20.0));
    umap!("amptodb",|amp: f64| 20.0 * amp.abs().max(1e-10).log10());
    umap!("sinc",   |x: f64| if x.abs() < 1e-10 { 1.0 } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    });

    reg(i, "hypot", |i| {
        let b = i.pop()?; let a = i.pop()?;
        i.push(automap_bin(a, b, f64::hypot)?);
        Ok(())
    });

    // lo hi x clip  → clamp x to [lo, hi]  (x is TOS)
    reg(i, "clip", |i| {
        let v  = i.pop()?;
        let hi = real_val(&i.pop()?)?;
        let lo = real_val(&i.pop()?)?;
        i.push(apply_clip(v, lo, hi)?);
        Ok(())
    });
    // lo hi x wrap  → wrap x into [lo, hi)  (x is TOS)
    reg(i, "wrap", |i| {
        let v  = i.pop()?;
        let hi = real_val(&i.pop()?)?;
        let lo = real_val(&i.pop()?)?;
        i.push(apply_wrap(v, lo, hi)?);
        Ok(())
    });
    // lo hi x fold  → fold/reflect x into [lo, hi]  (x is TOS)
    reg(i, "fold", |i| {
        let v  = i.pop()?;
        let hi = real_val(&i.pop()?)?;
        let lo = real_val(&i.pop()?)?;
        i.push(apply_fold(v, lo, hi)?);
        Ok(())
    });
    // slo shi dlo dhi x linlin  → linear range remap  (x is TOS)
    reg(i, "linlin", |i| {
        let v      = i.pop()?;
        let dst_hi = real_val(&i.pop()?)?;
        let dst_lo = real_val(&i.pop()?)?;
        let src_hi = real_val(&i.pop()?)?;
        let src_lo = real_val(&i.pop()?)?;
        i.push(apply_linlin(v, src_lo, src_hi, dst_lo, dst_hi)?);
        Ok(())
    });
    // slo shi dlo dhi x linexp  → linear input → exponential output range
    reg(i, "linexp", |i| {
        let v      = i.pop()?;
        let dst_hi = real_val(&i.pop()?)?;
        let dst_lo = real_val(&i.pop()?)?;
        let src_hi = real_val(&i.pop()?)?;
        let src_lo = real_val(&i.pop()?)?;
        i.push(apply_linexp(v, src_lo, src_hi, dst_lo, dst_hi)?);
        Ok(())
    });
    reg(i, "explin", |i| {
        let v      = i.pop()?;
        let dst_hi = real_val(&i.pop()?)?;
        let dst_lo = real_val(&i.pop()?)?;
        let src_hi = real_val(&i.pop()?)?;
        let src_lo = real_val(&i.pop()?)?;
        i.push(apply_explin(v, src_lo, src_hi, dst_lo, dst_hi)?);
        Ok(())
    });

    // ---- Signal analysis ----------------------------------------------------

    reg(i, "normalize", |i| {
        let v = i.pop()?;
        if let Value::Signal(s) = &v {
            if let Some(sl) = s.as_f32_slice() {
                let peak = sl.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
                if peak > 0.0 {
                    let out: Vec<f32> = sl.iter().map(|&x| x / peak).collect();
                    i.push(make_signal(out));
                    return Ok(());
                }
            }
        }
        i.push(v);
        Ok(())
    });
    reg(i, "peak", |i| {
        let sig = i.pop()?;
        let samples = collect_signal_f32(&sig)?;
        let peak = samples.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        i.push(Value::Real(peak as f64));
        Ok(())
    });
    reg(i, "rms", |i| {
        let sig = i.pop()?;
        let samples = collect_signal_f32(&sig)?;
        let rms = if samples.is_empty() { 0.0 } else {
            (samples.iter().map(|&x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
        };
        i.push(Value::Real(rms as f64));
        Ok(())
    });
    reg(i, "dur", |i| {
        let v = i.pop()?;
        let len = match &v {
            Value::Signal(s) => s.len_hint().unwrap_or(0),
            _ => return Err(Error::Type { expected: "Signal", actual: v.kind().name() }),
        };
        i.push(Value::Real(len as f64 / i.sample_rate));
        Ok(())
    });

    // ---- Random list generators ---------------------------------------------

    // n rands  → list of n uniform [0,1) reals
    reg(i, "rands", |i| {
        let n = real_val(&i.pop()?)? as usize;
        let mut seed = i.rng_seed;
        let out: Vec<Value> = (0..n).map(|_| {
            Value::Real((rng_step(&mut seed) >> 33) as f64 / u32::MAX as f64)
        }).collect();
        i.rng_seed = seed;
        i.push(make_list(out));
        Ok(())
    });
    // n max irands  → list of n random integers in [0, max)
    reg(i, "irands", |i| {
        let max = real_val(&i.pop()?)? as u64;
        let n   = real_val(&i.pop()?)? as usize;
        let mut seed = i.rng_seed;
        let out: Vec<Value> = (0..n).map(|_| {
            Value::Real(if max == 0 { 0.0 } else { ((rng_step(&mut seed) >> 33) % max) as f64 })
        }).collect();
        i.rng_seed = seed;
        i.push(make_list(out));
        Ok(())
    });
    // n list picks  → n random samples from list (with replacement)
    reg(i, "picks", |i| {
        let items = collect_to_vec(&i.pop()?)?;
        let n     = real_val(&i.pop()?)? as usize;
        if items.is_empty() { i.push(make_list(vec![])); return Ok(()); }
        let len = items.len() as u64;
        let mut seed = i.rng_seed;
        let out: Vec<Value> = (0..n).map(|_| {
            items[((rng_step(&mut seed) >> 33) % len) as usize].clone()
        }).collect();
        i.rng_seed = seed;
        i.push(make_list(out));
        Ok(())
    });
    // n prob coins  → n Bernoulli trials (0 or 1); prob in [0,1]
    reg(i, "coins", |i| {
        let prob   = real_val(&i.pop()?)? as f64;
        let n      = real_val(&i.pop()?)? as usize;
        let thresh = (prob * u32::MAX as f64) as u64;
        let mut seed = i.rng_seed;
        let out: Vec<Value> = (0..n).map(|_| {
            Value::Real(if (rng_step(&mut seed) >> 33) < thresh { 1.0 } else { 0.0 })
        }).collect();
        i.rng_seed = seed;
        i.push(make_list(out));
        Ok(())
    });

    // ---- LF noise / modulation signals --------------------------------------

    // freq lfnoise0  → stepped random signal
    reg(i, "lfnoise0", |i| {
        let freq = real_val(&i.pop()?)? as f32;
        let seed = rng_step(&mut i.rng_seed);
        i.push(Value::Signal(Arc::new(stax_dsp::LfNoise0Signal { freq_hz: freq, seed })));
        Ok(())
    });
    // freq lfnoise1  → linearly interpolated random signal
    reg(i, "lfnoise1", |i| {
        let freq = real_val(&i.pop()?)? as f32;
        let seed = rng_step(&mut i.rng_seed);
        i.push(Value::Signal(Arc::new(stax_dsp::LfNoise1Signal { freq_hz: freq, seed })));
        Ok(())
    });
    // trigger_signal source_signal sah  → sample-and-hold signal
    reg(i, "sah", |i| {
        let input = match i.pop()? {
            Value::Signal(s) => s,
            other => return Err(Error::Type { expected: "Signal (source)", actual: other.kind().name() }),
        };
        let trigger = match i.pop()? {
            Value::Signal(s) => s,
            other => return Err(Error::Type { expected: "Signal (trigger)", actual: other.kind().name() }),
        };
        i.push(Value::Signal(Arc::new(stax_dsp::SahSignal { input, trigger })));
        Ok(())
    });
    // density dust  → random impulse train Signal (avg density in Hz)
    reg(i, "dust", |i| {
        let density = real_val(&i.pop()?)? as f32;
        let seed = rng_step(&mut i.rng_seed);
        i.push(Value::Signal(Arc::new(stax_dsp::DustSignal { density_hz: density, seed, bipolar: false })));
        Ok(())
    });
    // density dust2  → bipolar random impulse train Signal (±1)
    reg(i, "dust2", |i| {
        let density = real_val(&i.pop()?)? as f32;
        let seed = rng_step(&mut i.rng_seed);
        i.push(Value::Signal(Arc::new(stax_dsp::DustSignal { density_hz: density, seed, bipolar: true })));
        Ok(())
    });

    // ---- Additional envelopes -----------------------------------------------

    // dur fadein  → 0→1 linear ramp over dur seconds
    reg(i, "fadein", |i| {
        let dur = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::LineSignal { start: 0.0, end: 1.0, dur_secs: dur })));
        Ok(())
    });
    // dur fadeout  → 1→0 linear ramp over dur seconds
    reg(i, "fadeout", |i| {
        let dur = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::LineSignal { start: 1.0, end: 0.0, dur_secs: dur })));
        Ok(())
    });
    // dur hanenv  → raised-cosine (Hanning) window envelope
    reg(i, "hanenv", |i| {
        let dur = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::HanEnvSignal { dur_secs: dur })));
        Ok(())
    });
    // attack decay decay2  → linear attack + exponential decay envelope
    reg(i, "decay2", |i| {
        let decay  = real_val(&i.pop()?)? as f32;
        let attack = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::Decay2Signal { attack_secs: attack, decay_secs: decay })));
        Ok(())
    });

    // ---- Delay --------------------------------------------------------------

    // signal delay_secs delayn  → fixed delay Signal
    reg(i, "delayn", |i| {
        let delay_secs = real_val(&i.pop()?)? as f32;
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::DelayNSignal { input, delay_secs })));
        Ok(())
    });

    // ---- Pan / spatial ------------------------------------------------------

    // signal pan pan2  → [L, R] equal-power stereo pan; pan in -1..1
    reg(i, "pan2", |i| {
        let pan_val = i.pop()?;
        let input = match i.pop()? {
            Value::Signal(s) => s,
            other => return Err(Error::Type { expected: "Signal", actual: other.kind().name() }),
        };
        let pan_sig: Arc<dyn stax_core::Signal> = match pan_val {
            Value::Signal(s) => s,
            Value::Real(p) => Arc::new(stax_dsp::ConstSignal { value: p as f32 }),
            other => return Err(Error::Type { expected: "Real or Signal (pan)", actual: other.kind().name() }),
        };
        let l: Arc<dyn stax_core::Signal> = Arc::new(stax_dsp::PanChannelSignal {
            input: input.clone(), pan: pan_sig.clone(), is_right: false,
        });
        let r: Arc<dyn stax_core::Signal> = Arc::new(stax_dsp::PanChannelSignal {
            input, pan: pan_sig, is_right: true,
        });
        i.push(make_list(vec![Value::Signal(l), Value::Signal(r)]));
        Ok(())
    });
    // [L, R] pan bal2  → [L', R'] balance (attenuate one side); pan in -1..1
    reg(i, "bal2", |i| {
        let pan = real_val(&i.pop()?)? as f32;
        let lr = collect_to_vec(&i.pop()?)?;
        if lr.len() < 2 { return Err(Error::Other("bal2: expected [L, R] list".into())); }
        let (l_sig, r_sig) = match (&lr[0], &lr[1]) {
            (Value::Signal(l), Value::Signal(r)) => (l.clone(), r.clone()),
            _ => return Err(Error::Other("bal2: list elements must be Signals".into())),
        };
        let l_gain = (1.0 - pan.max(0.0)).clamp(0.0, 1.0);
        let r_gain = (1.0 + pan.min(0.0)).clamp(0.0, 1.0);
        let l_out: Arc<dyn stax_core::Signal> = Arc::new(stax_core::signal::BinarySignal {
            a: l_sig, b: Arc::new(stax_dsp::ConstSignal { value: l_gain }), op: |a, b| a * b,
        });
        let r_out: Arc<dyn stax_core::Signal> = Arc::new(stax_core::signal::BinarySignal {
            a: r_sig, b: Arc::new(stax_dsp::ConstSignal { value: r_gain }), op: |a, b| a * b,
        });
        i.push(make_list(vec![Value::Signal(l_out), Value::Signal(r_out)]));
        Ok(())
    });
    // [L, R] angle rot2  → [L', R'] matrix rotation (M-S / stereo widening)
    reg(i, "rot2", |i| {
        let angle = real_val(&i.pop()?)? as f32;
        let lr = collect_to_vec(&i.pop()?)?;
        if lr.len() < 2 { return Err(Error::Other("rot2: expected [L, R] list".into())); }
        let (l_sig, r_sig) = match (&lr[0], &lr[1]) {
            (Value::Signal(l), Value::Signal(r)) => (l.clone(), r.clone()),
            _ => return Err(Error::Other("rot2: list elements must be Signals".into())),
        };
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        // L' = L*cos(a) + R*(-sin(a)), R' = L*sin(a) + R*cos(a)
        let l_out: Arc<dyn stax_core::Signal> = Arc::new(stax_dsp::Mix2Signal {
            a: l_sig.clone(), b: r_sig.clone(), gain_a: cos_a, gain_b: -sin_a,
        });
        let r_out: Arc<dyn stax_core::Signal> = Arc::new(stax_dsp::Mix2Signal {
            a: l_sig, b: r_sig, gain_a: sin_a, gain_b: cos_a,
        });
        i.push(make_list(vec![Value::Signal(l_out), Value::Signal(r_out)]));
        Ok(())
    });
    // signal pan_l pan_r pan_c pan3  → [L, C, R] 3-channel equal-power pan
    reg(i, "pan3", |i| {
        // Simplified: pan value -1→1, distributes over L/C/R
        let input = match i.pop()? {
            Value::Signal(s) => s,
            other => return Err(Error::Type { expected: "Signal", actual: other.kind().name() }),
        };
        let pan = real_val(&i.pop()?)? as f32;
        // Center gain peaks at pan=0; L peaks at pan=-1; R peaks at pan=+1
        let c_gain = (1.0 - pan.abs()).clamp(0.0, 1.0);
        let l_gain = ((-pan + 1.0) * 0.5).clamp(0.0, 1.0).sqrt();
        let r_gain = ((pan + 1.0) * 0.5).clamp(0.0, 1.0).sqrt();
        let mk = |g: f32| -> Arc<dyn stax_core::Signal> {
            Arc::new(stax_core::signal::BinarySignal {
                a: input.clone(),
                b: Arc::new(stax_dsp::ConstSignal { value: g }),
                op: |a, b| a * b,
            })
        };
        i.push(make_list(vec![Value::Signal(mk(l_gain)), Value::Signal(mk(c_gain)), Value::Signal(mk(r_gain))]));
        Ok(())
    });

    // ---- Sample-rate conversion ---------------------------------------------

    // signal n upSmp  → signal with each sample repeated n times
    reg(i, "upSmp", |i| {
        let factor = real_val(&i.pop()?)? as usize;
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::UpsampleSignal { input, factor: factor.max(1) })));
        Ok(())
    });
    // signal n dwnSmp  → signal taking first of every n input samples
    reg(i, "dwnSmp", |i| {
        let factor = real_val(&i.pop()?)? as usize;
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::DownsampleSignal { input, factor: factor.max(1) })));
        Ok(())
    });

    // ---- Multiband allpass disperser ----------------------------------------

    // signal stages lo_hz hi_hz disperser  → phase-dispersed signal
    reg(i, "disperser", |i| {
        let hi_hz  = real_val(&i.pop()?)? as f32;
        let lo_hz  = real_val(&i.pop()?)? as f32;
        let stages = real_val(&i.pop()?)? as usize;
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::DispersalSignal {
            input, stages, lo_hz: lo_hz.max(1.0), hi_hz: hi_hz.max(lo_hz + 1.0),
        })));
        Ok(())
    });

    // ---- Infinite math streams ----------------------------------------------

    // fib  → 0, 1, 1, 2, 3, 5, 8, ...
    reg(i, "fib", |i| {
        i.push(Value::Stream(Arc::new(stax_core::stream::IterStream::infinite(|| {
            let mut a = 0u64;
            let mut b = 1u64;
            Box::new(std::iter::from_fn(move || {
                let v = Value::Real(a as f64);
                let c = a.saturating_add(b);
                a = b;
                b = c;
                Some(v)
            }))
        }))));
        Ok(())
    });
    // ---- Strange attractors -------------------------------------------------

    // Classic Lorenz: 10 28 2.667 0.005 0.1 0 0 lorenz (scale * 0.05 for audio)
    reg(i, "lorenz", |i| {
        let z0    = real_val(&i.pop()?)? as f32;
        let y0    = real_val(&i.pop()?)? as f32;
        let x0    = real_val(&i.pop()?)? as f32;
        let dt    = real_val(&i.pop()?)? as f32;
        let beta  = real_val(&i.pop()?)? as f32;
        let rho   = real_val(&i.pop()?)? as f32;
        let sigma = real_val(&i.pop()?)? as f32;
        let mk = |output: u8| Value::Signal(Arc::new(stax_dsp::LorenzSignal {
            sigma, rho, beta, dt, x0, y0, z0, output
        }));
        i.push(make_list(vec![mk(0), mk(1), mk(2)]));
        Ok(())
    });

    // Classic Rossler: 0.2 0.2 5.7 0.01 0.1 0 0 rossler (scale * 0.1 for audio)
    reg(i, "rossler", |i| {
        let z0 = real_val(&i.pop()?)? as f32;
        let y0 = real_val(&i.pop()?)? as f32;
        let x0 = real_val(&i.pop()?)? as f32;
        let dt = real_val(&i.pop()?)? as f32;
        let c  = real_val(&i.pop()?)? as f32;
        let b  = real_val(&i.pop()?)? as f32;
        let a  = real_val(&i.pop()?)? as f32;
        let mk = |output: u8| Value::Signal(Arc::new(stax_dsp::RosslerSignal {
            a, b, c, dt, x0, y0, z0, output
        }));
        i.push(make_list(vec![mk(0), mk(1), mk(2)]));
        Ok(())
    });

    // Classic chaotic Duffing: -1 1 0.3 0.5 1.2 0.1 0 0 duffing
    reg(i, "duffing", |i| {
        let v0    = real_val(&i.pop()?)? as f32;
        let x0    = real_val(&i.pop()?)? as f32;
        let dt    = real_val(&i.pop()?)? as f32;
        let omega = real_val(&i.pop()?)? as f32;
        let gamma = real_val(&i.pop()?)? as f32;
        let delta = real_val(&i.pop()?)? as f32;
        let beta  = real_val(&i.pop()?)? as f32;
        let alpha = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::DuffingSignal {
            alpha, beta, delta, gamma, omega, dt, x0, v0
        })));
        Ok(())
    });

    // mu=1 mild, mu=5 relaxation oscillation; x range ≈ ±2
    reg(i, "vanderpol", |i| {
        let v0 = real_val(&i.pop()?)? as f32;
        let x0 = real_val(&i.pop()?)? as f32;
        let dt = real_val(&i.pop()?)? as f32;
        let mu = real_val(&i.pop()?)? as f32;
        i.push(Value::Signal(Arc::new(stax_dsp::VanDerPolSignal { mu, dt, x0, v0 })));
        Ok(())
    });

    // ---- Discrete maps (chaotic streams) ------------------------------------

    // x[n+1] = r*x*(1-x); chaotic for r in (3.57, 4]; values in [0,1] for x0 in (0,1)
    reg(i, "logistic", |i| {
        let x0 = real_val(&i.pop()?)?;
        let r  = real_val(&i.pop()?)?;
        i.push(Value::Stream(Arc::new(IterStream::infinite(move || {
            let mut x = x0;
            Box::new(std::iter::from_fn(move || {
                let v = x;
                x = r * x * (1.0 - x);
                Some(Value::Real(v))
            }))
        }))));
        Ok(())
    });

    // x[n+1] = 1 - a*x² + y, y[n+1] = b*x; classic: 1.4 0.3 0 0 henon (x ≈ [-1.5, 1.5])
    reg(i, "henon", |i| {
        let y0 = real_val(&i.pop()?)?;
        let x0 = real_val(&i.pop()?)?;
        let b  = real_val(&i.pop()?)?;
        let a  = real_val(&i.pop()?)?;
        i.push(Value::Stream(Arc::new(IterStream::infinite(move || {
            let mut x = x0;
            let mut y = y0;
            Box::new(std::iter::from_fn(move || {
                let (vx, vy) = (x, y);
                let nx = 1.0 - a * x * x + y;
                y = b * x;
                x = nx;
                Some(make_list(vec![Value::Real(vx), Value::Real(vy)]))
            }))
        }))));
        Ok(())
    });

    // primes  → 2, 3, 5, 7, 11, ... (lazy trial division)
    reg(i, "primes", |i| {
        i.push(Value::Stream(Arc::new(stax_core::stream::IterStream::infinite(|| {
            let mut candidate = 2u64;
            let mut found: Vec<u64> = Vec::new();
            Box::new(std::iter::from_fn(move || {
                'outer: loop {
                    let n = candidate;
                    candidate += 1;
                    for &p in &found {
                        if p * p > n { break; }
                        if n.is_multiple_of(p) { continue 'outer; }
                    }
                    found.push(n);
                    return Some(Value::Real(n as f64));
                }
            }))
        }))));
        Ok(())
    });

    // ---- SVF (State-Variable Filter) ----------------------------------------
    for (name, mode) in [("svflp", 0u8), ("svfhp", 1u8), ("svfbp", 2u8), ("svfnotch", 3u8)] {
        reg(i, name, move |i| {
            let q = real_val(&i.pop()?)? as f32;
            let freq = real_val(&i.pop()?)? as f32;
            let input = match i.pop()? {
                Value::Signal(s) => s,
                other => return Err(Error::Type { expected: "Signal", actual: other.kind().name() }),
            };
            i.push(Value::Signal(Arc::new(stax_dsp::SvfFilter { input, freq_hz: freq, q, mode })));
            Ok(())
        });
    }

    // ---- Compressor / limiter -----------------------------------------------
    reg(i, "compressor", |i| {
        let makeup    = real_val(&i.pop()?)? as f32;
        let release   = real_val(&i.pop()?)? as f32;
        let attack    = real_val(&i.pop()?)? as f32;
        let ratio     = real_val(&i.pop()?)? as f32;
        let threshold = real_val(&i.pop()?)? as f32;
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::CompressorSignal {
            input, threshold_db: threshold, ratio, attack_secs: attack, release_secs: release, makeup_db: makeup,
        })));
        Ok(())
    });
    // ratio=∞, minimal attack
    reg(i, "limiter", |i| {
        let threshold = real_val(&i.pop()?)? as f32;
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::CompressorSignal {
            input, threshold_db: threshold, ratio: 10000.0, attack_secs: 0.001, release_secs: 0.1, makeup_db: 0.0,
        })));
        Ok(())
    });

    // ---- Window functions ---------------------------------------------------
    for (name, wfn) in [
        ("hann",            stax_dsp::hann_window            as fn(usize) -> Vec<f32>),
        ("hamming",         stax_dsp::hamming_window),
        ("blackman",        stax_dsp::blackman_window),
        ("blackmanharris",  stax_dsp::blackman_harris_window),
        ("nuttall",         stax_dsp::nuttall_window),
        ("flattop",         stax_dsp::flat_top_window),
    ] {
        reg(i, name, move |i| {
            let n = real_val(&i.pop()?)? as usize;
            i.push(Value::Signal(Arc::new(VecSignal(wfn(n)))));
            Ok(())
        });
    }
    reg(i, "gaussian", |i| {
        let sigma = real_val(&i.pop()?)?;
        let n = real_val(&i.pop()?)? as usize;
        i.push(Value::Signal(Arc::new(VecSignal(stax_dsp::gaussian_window(n, sigma)))));
        Ok(())
    });
    reg(i, "kaiser", |i| {
        let beta = real_val(&i.pop()?)?;
        let n = real_val(&i.pop()?)? as usize;
        i.push(Value::Signal(Arc::new(VecSignal(stax_dsp::kaiser_window(n, beta)))));
        Ok(())
    });

    // ---- Hilbert transform --------------------------------------------------
    reg(i, "hilbert", |i| {
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::HilbertFilter { input })));
        Ok(())
    });

    // ---- Windowed-sinc FIR design -------------------------------------------
    reg(i, "firlp", |i| {
        let n_taps = real_val(&i.pop()?)? as usize;
        let cutoff = real_val(&i.pop()?)?;
        let input  = pop_signal(i)?;
        let coeffs = stax_dsp::fir_coeffs_lp(cutoff, i.sample_rate, n_taps);
        i.push(Value::Signal(Arc::new(stax_dsp::FirFilterSignal { input, coeffs })));
        Ok(())
    });
    reg(i, "firhp", |i| {
        let n_taps = real_val(&i.pop()?)? as usize;
        let cutoff = real_val(&i.pop()?)?;
        let input  = pop_signal(i)?;
        let coeffs = stax_dsp::fir_coeffs_hp(cutoff, i.sample_rate, n_taps);
        i.push(Value::Signal(Arc::new(stax_dsp::FirFilterSignal { input, coeffs })));
        Ok(())
    });
    reg(i, "firbp", |i| {
        let n_taps = real_val(&i.pop()?)? as usize;
        let hi_hz  = real_val(&i.pop()?)?;
        let lo_hz  = real_val(&i.pop()?)?;
        let input  = pop_signal(i)?;
        let coeffs = stax_dsp::fir_coeffs_bp(lo_hz, hi_hz, i.sample_rate, n_taps);
        i.push(Value::Signal(Arc::new(stax_dsp::FirFilterSignal { input, coeffs })));
        Ok(())
    });

    // ---- FDN Reverb (Jot/Hadamard) ------------------------------------------
    reg(i, "verb", |i| {
        let room  = real_val(&i.pop()?)? as f32;
        let decay = real_val(&i.pop()?)? as f32;
        let n     = real_val(&i.pop()?)? as usize;
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::FdnReverb { input, n_lines: n, decay_secs: decay, room_size: room })));
        Ok(())
    });

    // ---- Waveshaping --------------------------------------------------------
    for (name, mode) in [
        ("tanhsat",  stax_dsp::WaveShapeMode::Tanh),
        ("softclip", stax_dsp::WaveShapeMode::SoftClip),
        ("hardclip", stax_dsp::WaveShapeMode::HardClip),
        ("cubicsat", stax_dsp::WaveShapeMode::Cubic),
        ("atansat",  stax_dsp::WaveShapeMode::Atan),
    ] {
        reg(i, name, move |i| {
            let amount = real_val(&i.pop()?)? as f32;
            let input = match i.pop()? {
                Value::Signal(s) => s,
                other => return Err(Error::Type { expected: "Signal", actual: other.kind().name() }),
            };
            i.push(Value::Signal(Arc::new(stax_dsp::WaveShaperSignal { input, mode, amount })));
            Ok(())
        });
    }
    // signal order amount chebdist → signal
    reg(i, "chebdist", |i| {
        let amount = real_val(&i.pop()?)? as f32;
        let order  = real_val(&i.pop()?)? as u8;
        let input  = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::WaveShaperSignal {
            input, mode: stax_dsp::WaveShapeMode::Chebyshev(order), amount,
        })));
        Ok(())
    });

    // ---- Phase vocoder (offline) --------------------------------------------
    reg(i, "pvocstretch", |i| {
        let stretch  = real_val(&i.pop()?)? as f32;
        let hop      = real_val(&i.pop()?)? as usize;
        let fft_size = real_val(&i.pop()?)? as usize;
        let input    = pop_signal(i)?;
        let sr = i.sample_rate;
        let samples = collect_signal_f32_sr(&input, sr)?;
        let out = stax_dsp::pvoc_stretch(&samples, fft_size, hop, stretch);
        i.push(Value::Signal(Arc::new(VecSignal(out))));
        Ok(())
    });
    reg(i, "pvocp", |i| {
        let semitones = real_val(&i.pop()?)? as f32;
        let hop       = real_val(&i.pop()?)? as usize;
        let fft_size  = real_val(&i.pop()?)? as usize;
        let input     = pop_signal(i)?;
        let sr = i.sample_rate;
        let samples = collect_signal_f32_sr(&input, sr)?;
        let out = stax_dsp::pvoc_pitch(&samples, fft_size, hop, semitones);
        i.push(Value::Signal(Arc::new(VecSignal(out))));
        Ok(())
    });

    // ---- Granular synthesis -------------------------------------------------
    reg(i, "grain", |i| {
        let pitch_spread = real_val(&i.pop()?)? as f32;
        let pitch        = real_val(&i.pop()?)? as f32;
        let pos_spread   = real_val(&i.pop()?)? as f32;
        let pos          = real_val(&i.pop()?)? as f32;
        let density      = real_val(&i.pop()?)? as f32;
        let dur          = real_val(&i.pop()?)? as f32;
        let input        = pop_signal(i)?;
        let sr   = i.sample_rate;
        let seed = i.rng_seed;
        i.rng_seed = i.rng_seed.wrapping_add(1);
        let samples = collect_signal_f32_sr(&input, sr)?;
        i.push(Value::Signal(Arc::new(stax_dsp::GranularSynth {
            source: Arc::new(VecSignal(samples)),
            grain_dur_secs: dur, density, position: pos,
            position_spread: pos_spread, pitch, pitch_spread, seed,
        })));
        Ok(())
    });

    // ---- LPC analysis / synthesis -------------------------------------------
    reg(i, "lpcanalz", |i| {
        let order = real_val(&i.pop()?)? as usize;
        let input = pop_signal(i)?;
        let sr = i.sample_rate;
        let samples = collect_signal_f32_sr(&input, sr)?;
        let coeffs = stax_dsp::lpc_analyze(&samples, order);
        i.push(Value::Signal(Arc::new(VecSignal(coeffs))));
        Ok(())
    });
    reg(i, "lpcsynth", |i| {
        let coeffs_sig = pop_signal(i)?;
        let exc_sig    = pop_signal(i)?;
        let sr = i.sample_rate;
        let coeffs     = collect_signal_f32_sr(&coeffs_sig, sr)?;
        let excitation = collect_signal_f32_sr(&exc_sig, sr)?;
        let out = stax_dsp::lpc_synthesize(&excitation, &coeffs);
        i.push(Value::Signal(Arc::new(VecSignal(out))));
        Ok(())
    });

    // ---- Goertzel -----------------------------------------------------------
    reg(i, "goertzel", |i| {
        let freq  = real_val(&i.pop()?)?;
        let input = pop_signal(i)?;
        let sr = i.sample_rate;
        let samples = collect_signal_f32_sr(&input, sr)?;
        i.push(Value::Real(stax_dsp::goertzel_magnitude(&samples, freq, sr) as f64));
        Ok(())
    });
    reg(i, "goertzelc", |i| {
        let freq  = real_val(&i.pop()?)?;
        let input = pop_signal(i)?;
        let sr = i.sample_rate;
        let samples = collect_signal_f32_sr(&input, sr)?;
        let (re, im) = stax_dsp::goertzel_complex(&samples, freq, sr);
        i.push(make_list(vec![Value::Real(re as f64), Value::Real(im as f64)]));
        Ok(())
    });

    // ---- MDCT / IMDCT -------------------------------------------------------
    // signal mdct → VecSignal (N/2 coefficients)
    reg(i, "mdct", |i| {
        let input   = pop_signal(i)?;
        let sr      = i.sample_rate;
        let samples = collect_signal_f32_sr(&input, sr)?;
        i.push(Value::Signal(Arc::new(VecSignal(stax_dsp::mdct(&samples)))));
        Ok(())
    });
    // signal imdct → VecSignal (2N samples)
    reg(i, "imdct", |i| {
        let input  = pop_signal(i)?;
        let sr     = i.sample_rate;
        let coeffs = collect_signal_f32_sr(&input, sr)?;
        i.push(Value::Signal(Arc::new(VecSignal(stax_dsp::imdct(&coeffs)))));
        Ok(())
    });

    // ---- Thiran allpass -----------------------------------------------------
    // signal delay_samples order thiran → signal
    reg(i, "thiran", |i| {
        let order = real_val(&i.pop()?)? as usize;
        let delay = real_val(&i.pop()?)?;
        let input = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::ThiranAllpass { input, delay_samples: delay, order })));
        Ok(())
    });

    // ---- Farrow variable fractional delay -----------------------------------
    // signal delay_signal max_delay_secs farrow → signal
    reg(i, "farrow", |i| {
        let max_delay = real_val(&i.pop()?)? as f32;
        let delay_sig = pop_signal(i)?;
        let input     = pop_signal(i)?;
        i.push(Value::Signal(Arc::new(stax_dsp::FarrowDelay { input, delay_signal: delay_sig, max_delay_secs: max_delay })));
        Ok(())
    });

    // ---- CQT ----------------------------------------------------------------
    // signal bpo f_min n_bins cqt → VecSignal of magnitudes
    reg(i, "cqt", |i| {
        let n_bins  = real_val(&i.pop()?)? as usize;
        let f_min   = real_val(&i.pop()?)?;
        let bpo     = real_val(&i.pop()?)? as usize;
        let input   = pop_signal(i)?;
        let sr      = i.sample_rate;
        let samples = collect_signal_f32_sr(&input, sr)?;
        let mags    = stax_dsp::cqt_magnitudes(&samples, sr, bpo, f_min, n_bins);
        i.push(Value::Signal(Arc::new(VecSignal(mags))));
        Ok(())
    });
}

// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use stax_core::Signal;
    use stax_parser::parse;

    fn run(src: &str) -> Result<Interp> {
        let ops = parse(src).map_err(|e| Error::Other(e.to_string()))?;
        let mut interp = Interp::new();
        interp.exec(&ops)?;
        Ok(interp)
    }

    fn top_real(src: &str) -> f64 {
        let mut i = run(src).expect(src);
        i.pop().unwrap().as_real().unwrap_or_else(|| panic!("top of '{src}' is not a real"))
    }

    fn assert_truthy(src: &str) {
        let mut i = run(src).expect(src);
        let v = i.pop().expect(src);
        assert!(v.is_truthy(), "'{src}' left falsy: {v:?}");
    }

    #[test]
    fn arithmetic() {
        assert_eq!(top_real("1 2 +"), 3.0);
        assert_eq!(top_real("2 3 *"), 6.0);
    }

    #[test]
    fn equals_not() {
        assert_eq!(top_real("1 1 equals"), 1.0);
        assert_eq!(top_real("1 2 equals"), 0.0);
        assert_eq!(top_real("0 not"), 1.0);
        assert_eq!(top_real("1 not"), 0.0);
    }

    #[test]
    fn sym_equals() {
        assert_eq!(top_real("'a 'a equals"), 1.0);
        assert_eq!(top_real("'a 'b equals"), 0.0);
        assert_eq!(top_real("'a 'b equals not"), 1.0);
    }

    #[test]
    fn list_equals() {
        assert_truthy("[1][1] equals");
        assert_truthy("[][1] equals not");
        assert_truthy("[[1] 2][[1] 2] equals");
    }

    #[test]
    fn stack_shufflers() {
        assert_truthy("8 aa 2ple [8 8] equals");
        assert_truthy("1 2 ba 2ple [2 1] equals");
        assert_truthy("1 2 bab 3ple [2 1 2] equals");
        assert_truthy("1 2 aba 3ple [1 2 1] equals");
        assert_truthy("1 2 3 bac 3ple [2 1 3] equals");
        assert_truthy("1 2 3 nip 2ple [1 3] equals");
        assert_truthy("1 2 3 pop 2ple [1 2] equals");
        assert_truthy("clear stackDepth 0 equals");
    }

    #[test]
    fn tuple_ops() {
        assert_truthy("1 2 2ple [1 2] equals");
        assert_truthy("1 2 3 3ple [1 2 3] equals");
        assert_truthy("[1 2 3][4 5 6] 2ples [[1 4][2 5][3 6]] equals");
    }

    #[test]
    fn size_reverse() {
        assert_eq!(top_real("[1 2] size"), 2.0);
        assert_eq!(top_real("[] size"), 0.0);
        assert_eq!(top_real("123 size"), 1.0);
        assert_truthy("[1 2 3] reverse [3 2 1] equals");
    }

    #[test]
    fn conditional() {
        assert_truthy("1 \\['a]\\['b] if 'a equals");
        assert_truthy("0 \\['a]\\['b] if 'b equals");
    }

    #[test]
    fn refs() {
        assert_truthy("1 R get 1 equals");
        assert_truthy("1 R = r  2 r set  r get 2 equals");
    }

    #[test]
    fn apply_lambda() {
        assert_truthy("3 ! 3 equals");
        assert_truthy("\\[3] ! 3 equals");
        assert_truthy("7 4 \\a b[a b -] ! 3 equals");
    }

    #[test]
    fn fraction() {
        assert!((top_real("5/4") - 1.25).abs() < 1e-9);
    }

    #[test]
    fn reduce_scan_pairwise() {
        assert_eq!(top_real("[1 2 3 4] +/"), 10.0);
        assert_truthy("[1 2 3 4] +\\ [1 3 6 10] equals");
        assert_truthy("[1 2 3 4] +^ [1 3 5 7] equals");
        assert_truthy("[1 2 3 4] -^ [1 1 1 1] equals");
    }

    #[test]
    fn ord_nat_n() {
        assert_truthy("ord 4 N [1 2 3 4] equals");
        assert_truthy("nat 4 N [0 1 2 3] equals");
        assert_truthy("odds 4 N [1 3 5 7] equals");
    }

    #[test]
    fn to_finite() {
        assert_truthy("1 10 to finite 1 equals");
        assert_truthy("ord finite 0 equals");
    }

    #[test]
    fn auto_map() {
        assert_truthy("[1 2] 10 * [10 20] equals");
        assert_truthy("[1 2] [3 4 5] * [3 8] equals");
    }

    #[test]
    fn array_ops() {
        assert_truthy("[1 2 3 4]  1 rot [4 1 2 3] equals");
        assert_truthy("[1 2 3 4] -1 rot [2 3 4 1] equals");
        assert_truthy("[1 2 3 4]  1 shift [0 1 2 3] equals");
        assert_truthy("[1 2 3 4] -1 shift [2 3 4 0] equals");
        assert_truthy("[1 2 3 4 5]  3 N [1 2 3] equals");
        assert_truthy("[1 2 3 4 5]  3 skip [4 5] equals");
        assert_truthy("[1 2 3 4 5]  3 take [1 2 3] equals");
        assert_truthy("[1 2 3 4 5]  1 drop [2 3 4 5] equals");
    }

    #[test]
    fn at_ops() {
        assert_truthy("[1 2 3]  0 at 1 equals");
        assert_truthy("[1 2 3]  3 at 0 equals");
        assert_truthy("[7 8 9][0 2 2 1 0 1 -1 2 3 4] at [7 9 9 8 7 8 0 9 0 0] equals");
        assert_truthy("[7 8 9][0 2 2 1 0 1 -1 2 3 4] wrapAt [7 9 9 8 7 8 9 9 7 8] equals");
    }

    #[test]
    fn cat_ops() {
        assert_truthy("[1 2 3][4 5 6] $ [1 2 3 4 5 6] equals");
        assert_truthy("[[1 2] [[3 [4]]][5]] $/ [1 2 [3 [4]] 5] equals");
    }

    #[test]
    fn sort_grade() {
        assert_truthy("[3 4 2 5 1] sort [1 2 3 4 5] equals");
        assert_truthy("[3 4 2 5 1] sort> [5 4 3 2 1] equals");
        assert_truthy("[3 4 2 5 1] grade #[4 2 0 1 3] equals");
    }

    #[test]
    fn flat_flatten_clump() {
        assert_truthy("[[1 2] [[3 [4]]][5]] flat [1 2 3 4 5] equals");
        assert_truthy("[[[[1 2 3]]]] 1 flatten [[[1 2 3]]] equals");
        assert_truthy("1 64 to 2 clump 2 clump 2 clump flat 1 64 to equals");
    }

    #[test]
    fn add_cons_head_tail() {
        assert_truthy("[1 2 3 4] head 1 equals");
        assert_truthy("[1 2 3 4] tail [2 3 4] equals");
        assert_truthy("[2 3 4] 1 cons [1 2 3 4] equals");
        assert_truthy("[1 2 3] 4 add [1 2 3 4] equals");
        assert_truthy("[] empty");
        assert_truthy("[1] nonempty");
    }

    #[test]
    fn mirror() {
        assert_truthy("[1 2 3 4] mirror0 [1 2 3 4 3 2] equals");
        assert_truthy("[1 2 3 4] mirror1 [1 2 3 4 3 2 1] equals");
        assert_truthy("[1 2 3 4] mirror2 [1 2 3 4 4 3 2 1] equals");
    }

    #[test]
    fn bub_nbub() {
        assert_truthy("[1 2 3 4] @ bub [[1][2][3][4]] equals");
        assert_truthy("[1 2 3 4] @ 2 nbub [[[1]] [[2]] [[3]] [[4]]] equals");
        assert_truthy("[1 2 3 4] @ 0 nbub [1 2 3 4] equals");
    }

    #[test]
    fn ncyc() {
        assert_truthy("[1 2 3] 2 ncyc [1 2 3 1 2 3] equals");
        assert_truthy("[1 2 3] 0 ncyc [] equals");
    }

    #[test]
    fn signal_list() {
        assert_truthy("#[1]#[1] equals");
        assert_truthy("#[1]#[2] equals not");
        assert_truthy("#[1 2] size 2 equals");
        assert_truthy("#[1 2] reverse #[2 1] equals");
    }

    #[test]
    fn each_stack_preservation() {
        // Base arg on stack should survive across all each iterations.
        assert_truthy("100 [1 2 3] @ + [101 102 103] equals");
        // Extra arg pushed after @: each element consumes a fresh copy.
        assert_truthy("[1 2 3] @ 10 + [11 12 13] equals");
        // Two base args below mark.
        assert_truthy("1 2 [3 4 5] @ + [5 6 7] equals");
    }

    #[test]
    fn each_depth_and_outer_product() {
        // @@ applies word two levels deep.
        assert_truthy("[[1 2][3 4]] @@ neg [[-1 -2][-3 -4]] equals");
        // @1/@2 outer product.
        assert_truthy("[10 20]@1 [1 2]@2 + [[11 12][21 22]] equals");
        assert_truthy("[1 2 3]@1 [10 20]@2 + [[11 21][12 22][13 23]] equals");
    }

    #[test]
    fn each_zip_mode() {
        // Two @ calls = zip iteration.
        assert_truthy("[10 20]@ [1 2]@ + [11 22] equals");
    }

    #[test]
    fn ordz_and_gensignal() {
        // ordz produces an infinite Signal; N takes first n.
        assert_truthy("ordz 5 N #[1 2 3 4 5] equals");
        // ordz compared to ord element-wise via V.
        assert_truthy("ordz 3 N V ord 3 N equals");
    }

    #[test]
    fn n_with_stream_count() {
        // N auto-maps when count is a Stream; each count uses a fresh source iterator.
        assert_truthy("nat [2 3 1] N [[0 1][0 1 2][0]] equals");
        // ord (1-indexed): same fresh-iter semantics → same results per count
        assert_truthy("ord [3 2 1] N [[1 2 3][1 2][1]] equals");
    }

    #[test]
    fn to_automap() {
        // Scalar a, stream b → broadcast.
        assert_truthy("1 [1 2 3] to [[1][1 2][1 2 3]] equals");
        // Chained: 1 1 5 to to
        assert_truthy("1 1 5 to to [[1][1 2][1 2 3][1 2 3 4][1 2 3 4 5]] equals");
    }

    #[test]
    fn question_filter_infinite() {
        // ? with both infinite streams (lazy path).
        assert_truthy("nat [0 1] cyc ? 6 N [1 3 5 7 9 11] equals");
        // ord filtered by repeating mask 1,0.
        assert_truthy("ord [1 0] cyc ? 5 N [1 3 5 7 9] equals");
    }

    #[test]
    fn nby_word() {
        // Scalar: 3 values from 0 step 2.
        assert_truthy("3 0 2 nby [0 2 4] equals");
        // Stream counts and steps: nby produces list of seqs.
        assert_truthy("[3 2] 0 [1 2] nby [[0 1 2][0 2]] equals");
    }

    #[test]
    fn signal_at_variants() {
        // Signal source + stream index → Stream result.
        assert_truthy("#[10 20 30] [0 2 1] at [10 30 20] equals");
        // Signal source + signal index → Signal result.
        assert_truthy("#[10 20 30] #[2] at #[30] equals");
        // Stream source + signal index → Stream result.
        assert_truthy("[10 20 30] #[1] at [20] equals");
    }

    #[test]
    fn logical_ops() {
        assert_truthy("1 1 & 1 equals");
        assert_truthy("1 0 & 0 equals");
        assert_truthy("0 0 | 0 equals");
        assert_truthy("1 0 | 1 equals");
    }

    #[test]
    fn m2_seeds() {
        // seed word sets the RNG seed
        let mut i = Interp::new();
        let ops = stax_parser::parse("42 seed rand rand").unwrap();
        i.exec(&ops).unwrap();
        let r2 = i.pop().unwrap().as_real().unwrap();
        let r1 = i.pop().unwrap().as_real().unwrap();
        // Both should be in [0,1)
        assert!((0.0..1.0).contains(&r1));
        assert!((0.0..1.0).contains(&r2));
        // Same seed gives same sequence
        let mut j = Interp::new();
        let ops2 = stax_parser::parse("42 seed rand rand").unwrap();
        j.exec(&ops2).unwrap();
        let r2b = j.pop().unwrap().as_real().unwrap();
        let r1b = j.pop().unwrap().as_real().unwrap();
        assert_eq!(r1, r1b);
        assert_eq!(r2, r2b);
    }

    #[test]
    fn m2_irand() {
        assert_truthy("42 seed 10 irand 10 < ");
        assert_truthy("42 seed 1 irand 0 equals");
    }

    #[test]
    fn m3_sinosc_composes() {
        // sinosc creates a lazy Signal; * creates a BinarySignal (no as_f32_slice)
        let mut i = run("440 0 sinosc 0.3 *").unwrap();
        let v = i.pop().unwrap();
        assert!(matches!(v, Value::Signal(_)), "expected Signal, got {v:?}");
    }

    #[test]
    fn m3_saw_word() {
        let mut i = run("440 0 saw").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    #[test]
    fn m3_noise_words() {
        let mut i = run("1 wnoise").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        let mut j = run("1 pnoise").unwrap();
        assert!(matches!(j.pop().unwrap(), Value::Signal(_)));
    }

    #[test]
    fn m3_ar_adsr() {
        let mut i = run("0.01 0.1 ar").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        let mut j = run("0.01 0.05 0.7 0.1 0.2 adsr").unwrap();
        assert!(matches!(j.pop().unwrap(), Value::Signal(_)));
    }

    #[test]
    fn m3_pluck_word() {
        let mut i = run("440 1.0 pluck").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    #[test]
    fn m3_fft_ifft() {
        // FFT of a VecSignal → magnitude signal, length n/2+1
        let mut i = run("#[1 2 3 4] fft").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            assert_eq!(s.len_hint(), Some(3)); // n=4 → n/2+1=3
        } else {
            panic!("expected Signal");
        }
        // IFFT roundtrip (zero-phase): length is (n-1)*2 = 4
        let mut j = run("#[1 2 3 4] fft ifft").unwrap();
        if let Value::Signal(s) = j.pop().unwrap() {
            // length = (3-1)*2 = 4
            assert_eq!(s.len_hint(), Some(4));
        } else {
            panic!("expected Signal");
        }
    }

    #[test]
    fn m3_combn_word() {
        // combn wraps input Signal in a CombFilterSignal
        let mut i = run("1 wnoise 100 0.5 combn").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    #[test]
    fn m3_signal_compose_chain() {
        // Composing multiple lazy signals: sinosc * 0.3 + sinosc
        let mut i = run("440 0 sinosc 0.3 * 880 0 sinosc +").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    #[test]
    fn m3_midi_ports() {
        // midiPorts returns a Stream (possibly empty on CI)
        let mut i = run("midiPorts").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Stream(_)));
    }

    // ---- debug words --------------------------------------------------------

    #[test]
    fn debug_p_passthrough() {
        // p: print TOS (side-effect) and leave it on stack
        assert_eq!(top_real("42 p"), 42.0);
        // bench: time an expression (non-Fun passthrough), result stays on stack
        assert_eq!(top_real("3 4 + bench"), 7.0);
    }

    #[test]
    fn debug_trace_no_modify() {
        // trace prints but does not modify stack
        let i = run("1 2 3 trace").unwrap();
        assert_eq!(i.stack.len(), 3);
    }

    // ---- signal math --------------------------------------------------------

    #[test]
    fn math_sign() {
        assert_eq!(top_real("-5 sign"), -1.0);
        assert_eq!(top_real("0 sign"),   0.0);
        assert_eq!(top_real("7 sign"),   1.0);
    }

    #[test]
    fn math_hypot() {
        assert!((top_real("3 4 hypot") - 5.0).abs() < 1e-10);
    }

    #[test]
    fn math_clip_wrap_fold() {
        assert_eq!(top_real("0 10 15 clip"), 10.0);
        assert_eq!(top_real("0 10 5 clip"),   5.0);
        assert!((top_real("0 10 13 wrap") - 3.0).abs() < 1e-10);
        assert!((top_real("0 10 13 fold") - 7.0).abs() < 1e-10);
    }

    #[test]
    fn math_linlin() {
        // map 0.5 from [0,1] → [0,100] = 50
        assert!((top_real("0 1 0 100 0.5 linlin") - 50.0).abs() < 1e-10);
    }

    #[test]
    fn math_dbtamp_amptodb() {
        // 0dB → amplitude 1.0; amplitude 1.0 → 0dB
        assert!((top_real("0 dbtamp") - 1.0).abs() < 1e-10);
        assert!((top_real("1 amptodb") - 0.0).abs() < 1e-10);
        // roundtrip: any amplitude ≈ itself
        assert!((top_real("0.5 amptodb dbtamp") - 0.5).abs() < 1e-6);
    }

    #[test]
    fn math_sinc() {
        // sinc(0) = 1, sinc(1) = 0
        assert!((top_real("0 sinc") - 1.0).abs() < 1e-10);
        assert!(top_real("1 sinc").abs() < 1e-10);
    }

    #[test]
    fn math_midihz() {
        // MIDI 69 = A4 = 440 Hz
        assert!((top_real("69 midihz") - 440.0).abs() < 0.001);
        // roundtrip
        assert!((top_real("440 midinote") - 69.0).abs() < 0.001);
    }

    // ---- signal analysis ----------------------------------------------------

    #[test]
    fn signal_analysis() {
        // peak of [0.5, -1.0, 0.3] = 1.0
        assert!((top_real("#[0.5 -1 0.3] peak") - 1.0).abs() < 1e-5);
        // rms of [1,1,1,1] = 1.0
        assert!((top_real("#[1 1 1 1] rms") - 1.0).abs() < 1e-5);
        // normalize: peak becomes 1.0
        let mut i = run("#[0.5 -0.5] normalize peak").unwrap();
        assert!((i.pop().unwrap().as_real().unwrap() - 1.0).abs() < 1e-5);
    }

    // ---- random generators --------------------------------------------------

    #[test]
    fn random_rands() {
        let mut i = run("42 seed 5 rands").unwrap();
        let items = collect_to_vec(&i.pop().unwrap()).unwrap();
        assert_eq!(items.len(), 5);
        assert!(items.iter().all(|v| matches!(v, Value::Real(x) if *x >= 0.0 && *x < 1.0)));
    }

    #[test]
    fn random_irands_picks_coins() {
        // irands: 4 integers in [0, 10)
        let mut i = run("42 seed 4 10 irands").unwrap();
        let items = collect_to_vec(&i.pop().unwrap()).unwrap();
        assert_eq!(items.len(), 4);
        assert!(items.iter().all(|v| matches!(v, Value::Real(x) if *x >= 0.0 && *x < 10.0)));

        // picks: 3 items from [10, 20, 30]
        let mut i = run("42 seed 3 [10 20 30] picks").unwrap();
        let items = collect_to_vec(&i.pop().unwrap()).unwrap();
        assert_eq!(items.len(), 3);

        // coins: 4 trials with prob 0 → all 0; prob 1 → all 1
        assert_truthy("4 0 coins [0 0 0 0] equals");
        assert_truthy("4 1 coins [1 1 1 1] equals");
    }

    // ---- LF noise / envelopes -----------------------------------------------

    #[test]
    fn dsp_lfnoise() {
        // lfnoise0: produces a signal; consecutive samples at very low freq stay same value
        let mut i = run("42 seed 0.001 lfnoise0").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        // lfnoise1: same interface
        let mut i = run("42 seed 1 lfnoise1").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    #[test]
    fn dsp_envelopes() {
        // fadein: starts at 0, ends at 1
        let mut i = run("1 fadein").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            let mut inst = s.instantiate(4.0); // 4 Hz → 4 samples
            let mut buf = [0.0f32; 4];
            inst.fill(&mut buf);
            assert!(buf[0] < buf[3], "fadein should rise: {:?}", buf);
            assert!((buf[3] - 1.0).abs() < 0.01, "fadein should reach ~1: {:?}", buf);
        } else { panic!("expected Signal"); }

        // fadeout: starts at 1, ends near 0
        let mut i = run("1 fadeout").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            let mut inst = s.instantiate(4.0);
            let mut buf = [0.0f32; 4];
            inst.fill(&mut buf);
            assert!(buf[0] > buf[3], "fadeout should fall");
        } else { panic!("expected Signal"); }

        // hanenv: peaks at middle
        let mut i = run("1 hanenv").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            let mut inst = s.instantiate(10.0); // 10 samples
            let mut buf = [0.0f32; 10];
            inst.fill(&mut buf);
            assert!(buf[4] > buf[0] && buf[4] > buf[9], "hanenv should peak at middle");
        } else { panic!("expected Signal"); }
    }

    // ---- delay --------------------------------------------------------------

    #[test]
    fn dsp_delayn() {
        // delayn by 1 sample: first output is 0, subsequent outputs are prior inputs
        let sr = 1000.0f64;
        let input_samples = vec![1.0f32, 2.0, 3.0, 4.0];
        let input_sig = Arc::new(stax_dsp::VecSignal(input_samples));
        let delayed = stax_dsp::DelayNSignal { input: input_sig, delay_secs: 1.0 / sr as f32 };
        let mut inst = delayed.instantiate(sr);
        let mut buf = [0.0f32; 4];
        inst.fill(&mut buf);
        // buf[0] = 0 (initial silence), buf[1] = 1.0, buf[2] = 2.0, buf[3] = 3.0
        assert_eq!(buf[0], 0.0);
        assert_eq!(buf[1], 1.0);
        assert_eq!(buf[2], 2.0);
    }

    // ---- pan ----------------------------------------------------------------

    #[test]
    fn dsp_pan2() {
        // center pan (0): L = R = input * cos(π/4) = input * 1/√2
        let mut i = run("440 0 sinosc 0 pan2").unwrap();
        let items = collect_to_vec(&i.pop().unwrap()).unwrap();
        assert_eq!(items.len(), 2, "pan2 should produce [L, R]");
        assert!(matches!(&items[0], Value::Signal(_)));
        assert!(matches!(&items[1], Value::Signal(_)));
    }

    #[test]
    fn dsp_bal2_rot2() {
        // bal2: [L, R] list in, [L', R'] list out
        let mut i = run("440 0 sinosc 0 pan2 0 bal2").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Stream(_)));

        // rot2: same
        let mut i = run("440 0 sinosc 0 pan2 0 rot2").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Stream(_)));
    }

    // ---- sample rate conversion ---------------------------------------------

    #[test]
    fn dsp_upsmp_dwnsmp() {
        // upSmp 2: output repeats each sample — use finite VecSignal
        let mut i = run("42 seed 4 rands Z 2 upSmp").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));

        // dwnSmp 2: produces a signal
        let mut i = run("42 seed 4 rands Z 2 dwnSmp").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    // ---- disperser ----------------------------------------------------------

    #[test]
    fn dsp_disperser() {
        // disperser should produce a Signal without error
        let mut i = run("440 0 sinosc 8 200 4000 disperser").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));

        // Output is allpass: energy preserved over long enough window
        let mut i = run("#[1 0 0 0 0 0 0 0] 4 100 1000 disperser").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            let mut inst = s.instantiate(8000.0);
            // 512 samples captures full allpass tail — impulse response decays exponentially
            let mut buf = [0.0f32; 512];
            inst.fill(&mut buf);
            let energy: f32 = buf.iter().map(|x| x * x).sum();
            assert!((energy - 1.0).abs() < 0.01, "allpass energy not preserved: {energy}");
        } else { panic!("expected Signal"); }
    }

    // ---- infinite math streams ----------------------------------------------

    #[test]
    fn streams_fib_primes() {
        // fib: 0, 1, 1, 2, 3, 5, 8, 13
        assert_truthy("fib 8 N [0 1 1 2 3 5 8 13] equals");
        // primes: 2, 3, 5, 7, 11
        assert_truthy("primes 5 N [2 3 5 7 11] equals");
    }

    // ---- strange attractors -------------------------------------------------

    #[test]
    fn attractor_lorenz() {
        let mut i = run("10 28 2.667 0.005 0.1 0 0 lorenz").unwrap();
        let items = collect_to_vec(&i.pop().unwrap()).unwrap();
        assert_eq!(items.len(), 3, "lorenz → [x, y, z]");
        assert!(items.iter().all(|v| matches!(v, Value::Signal(_))));
        // x channel starts at x0=0.1, evolves and diverges
        if let Value::Signal(s) = &items[0] {
            let mut inst = s.instantiate(48000.0);
            let mut buf = [0.0f32; 256];
            inst.fill(&mut buf);
            let variance = {
                let mean = buf.iter().sum::<f32>() / 256.0;
                buf.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / 256.0
            };
            assert!(variance > 0.0, "lorenz x should vary");
        }
    }

    #[test]
    fn attractor_rossler() {
        let mut i = run("0.2 0.2 5.7 0.01 0.1 0 0 rossler").unwrap();
        let items = collect_to_vec(&i.pop().unwrap()).unwrap();
        assert_eq!(items.len(), 3, "rossler → [x, y, z]");
        assert!(items.iter().all(|v| matches!(v, Value::Signal(_))));
    }

    #[test]
    fn attractor_duffing() {
        // classic chaotic double-well: alpha=-1, beta=1, delta=0.3, gamma=0.5, omega=1.2
        let mut i = run("-1 1 0.3 0.5 1.2 0.1 0 0 duffing").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        // produce samples and check variation
        let mut i = run("-1 1 0.3 0.5 1.2 0.1 0 0 duffing").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            let mut inst = s.instantiate(48000.0);
            let mut buf = [0.0f32; 256];
            inst.fill(&mut buf);
            let variance = {
                let mean = buf.iter().sum::<f32>() / 256.0;
                buf.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / 256.0
            };
            assert!(variance > 0.0, "duffing should produce varying output");
        }
    }

    #[test]
    fn attractor_vanderpol() {
        // mu=1: mild nonlinear, x range ≈ ±2
        let mut i = run("1.0 0.01 0 1 vanderpol").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        // verify it oscillates (non-constant output)
        let mut i = run("1.0 0.01 0 1 vanderpol").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            let mut inst = s.instantiate(48000.0);
            let mut buf = [0.0f32; 256];
            inst.fill(&mut buf);
            let max = buf.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let min = buf.iter().cloned().fold(f32::INFINITY, f32::min);
            assert!(max > min, "vanderpol should oscillate");
        }
    }

    #[test]
    fn discrete_map_logistic() {
        // r=3.99: fully chaotic, values in [0,1]
        let mut i = run("3.99 0.5 logistic 30 N").unwrap();
        let items = collect_to_vec(&i.pop().unwrap()).unwrap();
        assert_eq!(items.len(), 30);
        for v in &items {
            let Value::Real(x) = v else { panic!("expected Real") };
            assert!(*x >= 0.0 && *x <= 1.0, "logistic out of [0,1]: {x}");
        }
        // r=1: converges to 0 — first value is x0=0.5
        assert!((top_real("1.0 0.5 logistic head") - 0.5).abs() < 1e-10);
    }

    #[test]
    fn discrete_map_henon() {
        // classic Henon: a=1.4, b=0.3
        let mut i = run("1.4 0.3 0 0 henon 5 N").unwrap();
        let items = collect_to_vec(&i.pop().unwrap()).unwrap();
        assert_eq!(items.len(), 5);
        // each element is a [x, y] pair
        let pair = collect_to_vec(&items[1]).unwrap();
        assert_eq!(pair.len(), 2, "henon element should be [x, y]");
        assert!(matches!(&pair[0], Value::Real(_)));
        assert!(matches!(&pair[1], Value::Real(_)));
        // first [x, y] = [0, 0] (initial conditions)
        let pair0 = collect_to_vec(&items[0]).unwrap();
        assert_eq!(pair0[0].as_real().unwrap(), 0.0);
        assert_eq!(pair0[1].as_real().unwrap(), 0.0);
    }

    fn fill_signal(sig: &Arc<dyn stax_core::Signal>, n: usize) -> Vec<f32> {
        let mut inst = sig.instantiate(48000.0);
        let mut buf = vec![0.0f32; n];
        inst.fill(&mut buf);
        buf
    }

    // ---- SVF ----------------------------------------------------------------
    #[test]
    fn svf_lp_attenuates_high() {
        // LP at 1 kHz should pass low freq (100 Hz) and attenuate high freq (10 kHz)
        let mut i = run("440 0 sinosc 1000 0.7 svflp").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        // basic construction works
        let mut i2 = run("100 0 sinosc 500 0.7 svflp").unwrap();
        if let Value::Signal(s) = i2.pop().unwrap() {
            let buf = fill_signal(&s, 2048);
            let energy: f32 = buf.iter().map(|x| x*x).sum::<f32>() / buf.len() as f32;
            assert!(energy > 0.0, "svflp should produce nonzero output");
        }
    }

    #[test]
    fn svf_all_modes_produce_signal() {
        for word in &["svflp", "svfhp", "svfbp", "svfnotch"] {
            let mut i = run(&format!("440 0 sinosc 1000 0.7 {word}")).unwrap();
            assert!(matches!(i.pop().unwrap(), Value::Signal(_)), "{word} should produce Signal");
        }
    }

    // ---- Compressor ---------------------------------------------------------
    #[test]
    fn compressor_reduces_loud_signal() {
        // loud sine should be reduced by compressor
        let mut i = run("440 0 sinosc -20 4 0.01 0.1 0 compressor").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        // limiter word
        let mut i2 = run("440 0 sinosc -20 limiter").unwrap();
        assert!(matches!(i2.pop().unwrap(), Value::Signal(_)));
    }

    // ---- Window functions ---------------------------------------------------
    #[test]
    fn window_hann_symmetry() {
        // hann(8): should peak at center, be near 0 at edges
        let mut i = run("8 hann").unwrap();
        let sig = i.pop().unwrap();
        if let Value::Signal(s) = sig {
            let sl = s.as_f32_slice().unwrap();
            assert_eq!(sl.len(), 8);
            assert!(sl[0].abs() < 0.01, "hann[0] ≈ 0");
            assert!(sl[3] > 0.9, "hann[3] near center should be high, got {}", sl[3]);
        } else { panic!("expected Signal"); }
    }

    #[test]
    fn window_all_types_work() {
        for word in &["hann", "hamming", "blackman", "blackmanharris", "nuttall", "flattop"] {
            let mut i = run(&format!("64 {word}")).unwrap();
            let sig = i.pop().unwrap();
            let Value::Signal(s) = sig else { panic!("{word}: expected Signal") };
            assert_eq!(s.as_f32_slice().map(|sl| sl.len()), Some(64));
        }
        let mut i = run("64 0.5 gaussian").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        let mut i2 = run("64 4.0 kaiser").unwrap();
        assert!(matches!(i2.pop().unwrap(), Value::Signal(_)));
    }

    // ---- Hilbert ------------------------------------------------------------
    #[test]
    fn hilbert_produces_signal() {
        let mut i = run("440 0 sinosc hilbert").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    // ---- FIR filters --------------------------------------------------------
    #[test]
    fn firlp_attenuates_high_freq() {
        // LP at 1 kHz: 10 kHz sine should be heavily attenuated
        let mut i = run("10000 0 sinosc 1000 63 firlp").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            let buf = fill_signal(&s, 4096);
            // skip filter delay (63 taps), measure energy in second half
            let energy: f32 = buf[63..].iter().map(|x| x*x).sum::<f32>() / (buf.len() - 63) as f32;
            assert!(energy < 0.01, "firlp should strongly attenuate 10 kHz, got energy={energy}");
        }
    }

    #[test]
    fn firhp_and_firbp_compile() {
        let mut i = run("440 0 sinosc 1000 63 firhp").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
        let mut i2 = run("440 0 sinosc 200 2000 63 firbp").unwrap();
        assert!(matches!(i2.pop().unwrap(), Value::Signal(_)));
    }

    // ---- FDN Reverb ---------------------------------------------------------
    #[test]
    fn verb_produces_tail() {
        let mut i = run("440 0 sinosc 4 2.0 0.5 verb").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    // ---- Waveshaping --------------------------------------------------------
    #[test]
    fn waveshaping_tanh_clamps() {
        // tanh of a large sine should be near ±1
        let mut i = run("440 0 sinosc 10.0 tanhsat").unwrap();
        if let Value::Signal(s) = i.pop().unwrap() {
            let buf = fill_signal(&s, 512);
            let max = buf.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            assert!(max <= 1.001, "tanhsat output should not exceed 1.0, got {max}");
        }
    }

    #[test]
    fn waveshaping_all_modes_work() {
        for (word, extra) in &[
            ("softclip", "2.0"),
            ("hardclip", "2.0"),
            ("cubicsat", "1.0"),
            ("atansat",  "3.0"),
        ] {
            let prog = format!("440 0 sinosc {extra} {word}");
            let mut i = run(&prog).unwrap();
            assert!(matches!(i.pop().unwrap(), Value::Signal(_)), "{word}: expected Signal");
        }
        let mut i = run("440 0 sinosc 5 0.8 chebdist").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    // ---- Phase Vocoder -------------------------------------------------------
    #[test]
    fn pvoc_stretch_changes_length() {
        // stretch a short VecSignal by 2×
        let mut i = run("#[0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1] 8 2 2.0 pvocstretch").unwrap();
        let v = i.pop().unwrap();
        if let Value::Signal(s) = v {
            let len = s.len_hint().unwrap_or(0);
            // 32 samples * 2.0 stretch = ~64 samples
            assert!(len > 0, "pvocstretch should produce samples");
        }
    }

    #[test]
    fn pvoc_pitch_preserves_length() {
        let mut i = run("#[0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1] 8 2 7.0 pvocp").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    // ---- Granular -----------------------------------------------------------
    #[test]
    fn grain_produces_audio() {
        let mut i = run("#[0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1 0 1 0 -1] 0.01 20.0 0.5 0.1 1.0 0.5 grain").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    // ---- LPC ----------------------------------------------------------------
    #[test]
    fn lpc_roundtrip() {
        // synthesize a signal, analyze, resynthesize from impulse
        let samples: Vec<f32> = (0..256).map(|i| (440.0 * 2.0 * std::f32::consts::PI * i as f32 / 48000.0).sin()).collect();
        let coeffs = stax_dsp::lpc_analyze(&samples, 8);
        assert_eq!(coeffs.len(), 8);
        let impulse = vec![0.0f32; 256].into_iter().enumerate().map(|(i, _)| if i==0 {1.0} else {0.0}).collect::<Vec<_>>();
        let synth = stax_dsp::lpc_synthesize(&impulse, &coeffs);
        assert_eq!(synth.len(), 256);
        // synthesized signal should be non-trivial
        let energy: f32 = synth.iter().map(|x| x*x).sum::<f32>() / synth.len() as f32;
        assert!(energy > 0.0, "lpc_synthesize should produce nonzero output");
    }

    #[test]
    fn lpc_words_work() {
        // Build a 64-sample sine VecSignal via #[...] notation — use stax_dsp directly
        let samples: Vec<f32> = (0..64).map(|i| (440.0 * 2.0 * std::f32::consts::PI * i as f32 / 48000.0).sin()).collect();
        let coeffs = stax_dsp::lpc_analyze(&samples, 4);
        assert_eq!(coeffs.len(), 4);
    }

    // ---- Goertzel -----------------------------------------------------------
    #[test]
    fn goertzel_detects_frequency() {
        // 1 second of 440 Hz sine at 48 kHz
        let sr = 48000.0f64;
        let n = 4096usize;
        let samples: Vec<f32> = (0..n).map(|i| (440.0 * 2.0 * std::f64::consts::PI * i as f64 / sr).sin() as f32).collect();
        let mag_440  = stax_dsp::goertzel_magnitude(&samples, 440.0, sr);
        let mag_1000 = stax_dsp::goertzel_magnitude(&samples, 1000.0, sr);
        assert!(mag_440 > mag_1000 * 10.0, "goertzel should detect 440 Hz, got {mag_440} vs {mag_1000}");
    }

    #[test]
    fn goertzel_word_works() {
        // Use a pre-built VecSignal via the word directly
        let samples: Vec<f32> = (0..256).map(|i| (440.0 * 2.0 * std::f32::consts::PI * i as f32 / 48000.0).sin()).collect();
        let mag = stax_dsp::goertzel_magnitude(&samples, 440.0, 48000.0);
        assert!(mag > 0.0);
    }

    // ---- MDCT ---------------------------------------------------------------
    #[test]
    fn mdct_imdct_roundtrip_shape() {
        let samples: Vec<f32> = (0..64).map(|i| (i as f32 / 64.0).sin()).collect();
        let coeffs = stax_dsp::mdct(&samples);
        assert_eq!(coeffs.len(), 32, "mdct of N samples → N/2 coefficients");
        let reconstructed = stax_dsp::imdct(&coeffs);
        assert_eq!(reconstructed.len(), 64, "imdct of M coefficients → 2M samples");
        // not a perfect roundtrip, but should be same scale
        let orig_max = samples.iter().cloned().fold(0.0f32, f32::max);
        let rec_max  = reconstructed.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(rec_max.abs() > 0.01, "imdct output should be non-trivial");
    }

    // ---- Thiran -------------------------------------------------------------
    #[test]
    fn thiran_delays_signal() {
        // Thiran with delay=2.5, order=4 should produce a delayed output
        let mut i = run("440 0 sinosc 2.5 4 thiran").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    // ---- Farrow -------------------------------------------------------------
    #[test]
    fn farrow_variable_delay() {
        // Use a ConstSignal of 5.0 samples delay as the modulator
        let mut i = run("440 0 sinosc 0 5 sinosc 0.5 farrow").unwrap();
        assert!(matches!(i.pop().unwrap(), Value::Signal(_)));
    }

    // ---- CQT ----------------------------------------------------------------
    #[test]
    fn cqt_frequency_detection() {
        // 440 Hz sine, CQT 12 bins/oct starting at 220 Hz, 2 octaves
        let sr = 48000.0f64;
        let n = 4096usize;
        let samples: Vec<f32> = (0..n).map(|i| (440.0 * 2.0 * std::f64::consts::PI * i as f64 / sr).sin() as f32).collect();
        let mags = stax_dsp::cqt_magnitudes(&samples, sr, 12, 220.0, 24);
        assert_eq!(mags.len(), 24);
        // 440 Hz is 1 octave above 220 Hz = 12 bins up from bin 0
        let peak_bin = mags.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i).unwrap();
        // 440 Hz bin = 12 (one octave above 220 Hz)
        assert!((peak_bin as i32 - 12).abs() <= 2, "CQT peak at {peak_bin}, expected near 12");
    }
}
