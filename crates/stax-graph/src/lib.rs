//! Graph IR and bidirectional Op ↔ graph round-trip (M4).
//!
//! The graph is a *view* over the Op stream — never a separate artifact.
//!
//! # Invariant
//! `lower(lift(ops))` produces the same `Vec<Op>` as the original for all
//! programs emitted by `stax-parser`. Evaluation of the lowered ops is
//! semantically identical to evaluation of the originals.
//!
//! # Design
//! Nodes are stored in insertion order (the order the corresponding ops
//! appeared in the source). `lower` emits nodes in that same order, making
//! the round-trip identity trivially correct. `topo_sort` is provided as a
//! utility for the M5 editor where users reorder nodes.

use std::collections::HashMap;
use std::sync::Arc;

use stax_core::{op::Adverb, Op, Value, ValueKind};

// ── IDs ──────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct NodeId(pub u32);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct EdgeId(pub u32);

/// Reference to a specific port on a node.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct PortRef {
    pub node: NodeId,
    pub port: u8,
}

// ── Port ─────────────────────────────────────────────────────────────────────

/// Expected value kind for a port (informational — drives port colour in the editor).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub enum PortKind {
    #[default]
    Any,
    Real,
    Signal,
    Stream,
    Fun,
    Form,
    Str,
    Sym,
}

impl From<ValueKind> for PortKind {
    fn from(k: ValueKind) -> Self {
        match k {
            ValueKind::Real   => PortKind::Real,
            ValueKind::Signal => PortKind::Signal,
            ValueKind::Stream => PortKind::Stream,
            ValueKind::Fun    => PortKind::Fun,
            ValueKind::Form   => PortKind::Form,
            ValueKind::Str    => PortKind::Str,
            ValueKind::Sym    => PortKind::Sym,
            _                 => PortKind::Any,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Port {
    pub kind: PortKind,
    pub label: Arc<str>,
    /// Rank-lift depth: 0 = none, 1 = @, 2 = @@, 3 = @@@
    pub each_depth: u8,
    /// Outer-product order tag: 0 = unordered, 1 = @1, 2 = @2
    pub each_order: u8,
}

impl Port {
    fn any() -> Self {
        Port { kind: PortKind::Any, label: Arc::from(""), each_depth: 0, each_order: 0 }
    }
    pub fn with_each(mut self, depth: u8, order: u8) -> Self {
        self.each_depth = depth;
        self.each_order = order;
        self
    }
}

// ── Node ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    /// Layout position — (0, 0) for headless; set by the editor in M5.
    pub pos: [f32; 2],
    /// Adverb modifier (reduce / scan / pairwise). Attached from a preceding `Adverb` op.
    pub adverb: Option<Adverb>,
}

impl Node {
    pub fn label(&self) -> String {
        self.kind.label()
    }

    /// True for nodes that source values (no data inputs, only produce).
    pub fn is_source(&self) -> bool {
        self.inputs.is_empty() && !self.outputs.is_empty()
    }

    /// True for nodes that sink values (consume but don't produce stack outputs).
    pub fn is_sink(&self) -> bool {
        self.outputs.is_empty()
    }
}

#[derive(Clone, Debug)]
pub enum NodeKind {
    /// A literal constant pushed onto the stack.
    Literal(Value),
    /// A built-in or user-defined word.
    Word(Arc<str>),
    /// `` `word `` — push binding without calling.
    Quote(Arc<str>),
    /// `'word` — push the symbol itself.
    Sym(Arc<str>),
    /// `,word` — pop a Form, look up `word`, push the value.
    FormGet(Arc<str>),
    /// `.word` — pop a Form, look up `word`, call it.
    FormApply(Arc<str>),
    /// `= word` — bind TOS to `word` in the current scope.
    Bind(Arc<str>),
    /// `= (a b c)` / `= [a b c]` — multi-bind.
    BindMany { names: Arc<[Arc<str>]>, list_mode: bool },
    /// `!` — call the function on TOS.
    Call,
    /// `[` — records stack depth for list construction.
    ListMark,
    /// `]` — collect back to `[`, wrap values into a list/signal-list.
    MakeList { signal: bool },
    /// `{ :key val }` — build a Form.
    MakeForm { keys: Arc<[Arc<str>]>, parent: bool },
    /// `\params [body]` — build a Function.
    MakeFun { params: Arc<[Arc<str>]>, body: Arc<[Op]> },
}

impl NodeKind {
    pub fn label(&self) -> String {
        match self {
            NodeKind::Literal(v)  => format!("{v:?}"),
            NodeKind::Word(w)     => w.to_string(),
            NodeKind::Quote(w)    => format!("`{w}"),
            NodeKind::Sym(w)      => format!("'{w}"),
            NodeKind::FormGet(w)  => format!(",{w}"),
            NodeKind::FormApply(w)=> format!(".{w}"),
            NodeKind::Bind(w)     => format!("={w}"),
            NodeKind::BindMany { names, .. } => {
                let ns: Vec<&str> = names.iter().map(|s| s.as_ref()).collect();
                format!("=({})", ns.join(" "))
            }
            NodeKind::Call                    => "!".into(),
            NodeKind::ListMark                => "[".into(),
            NodeKind::MakeList { signal }     => if *signal { "#]" } else { "]" }.into(),
            NodeKind::MakeForm { keys, .. }   => {
                let ks: Vec<&str> = keys.iter().map(|s| s.as_ref()).collect();
                format!("{{{}}}", ks.join(" "))
            }
            NodeKind::MakeFun { params, .. }  => {
                if params.is_empty() { "\\[]".into() }
                else {
                    let ps: Vec<&str> = params.iter().map(|s| s.as_ref()).collect();
                    format!("\\[{}]", ps.join(" "))
                }
            }
        }
    }
}

// ── Edge ─────────────────────────────────────────────────────────────────────

/// A directed data-flow edge from an output port to an input port.
/// Each edge represents one stack slot being passed from producer to consumer.
#[derive(Clone, Debug)]
pub struct Edge {
    pub id: EdgeId,
    pub src: PortRef,
    pub dst: PortRef,
}

// ── Graph ────────────────────────────────────────────────────────────────────

/// The stax graph IR.
///
/// A DAG of nodes (operations) connected by data-flow edges (stack slots).
/// `node_order` records insertion order, which mirrors the original Op stream
/// order and is used as the canonical lowering order.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    nodes: HashMap<NodeId, Node>,
    node_order: Vec<NodeId>,
    edges: Vec<Edge>,
    next_node: u32,
    next_edge: u32,
}

impl Graph {
    pub fn new() -> Self { Self::default() }

    // ── Construction ─────────────────────────────────────────────────────────

    pub fn add_node(&mut self, kind: NodeKind, n_in: usize, n_out: usize) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node += 1;
        let node = Node {
            id,
            kind,
            inputs:  (0..n_in).map(|_| Port::any()).collect(),
            outputs: (0..n_out).map(|_| Port::any()).collect(),
            pos: [0.0, 0.0],
            adverb: None,
        };
        self.nodes.insert(id, node);
        self.node_order.push(id);
        id
    }

    pub fn connect(&mut self, src: PortRef, dst: PortRef) -> EdgeId {
        let id = EdgeId(self.next_edge);
        self.next_edge += 1;
        self.edges.push(Edge { id, src, dst });
        id
    }

    // ── Queries ──────────────────────────────────────────────────────────────

    pub fn node(&self, id: NodeId) -> Option<&Node>     { self.nodes.get(&id) }
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> { self.nodes.get_mut(&id) }

    pub fn nodes_in_order(&self) -> impl Iterator<Item = &Node> {
        self.node_order.iter().filter_map(|id| self.nodes.get(id))
    }

    pub fn edges(&self) -> &[Edge] { &self.edges }
    pub fn node_count(&self) -> usize { self.nodes.len() }
    pub fn edge_count(&self) -> usize { self.edges.len() }

    /// The output `PortRef` that feeds a given input port, if any.
    pub fn source_of(&self, dst: PortRef) -> Option<PortRef> {
        self.edges.iter().find(|e| e.dst == dst).map(|e| e.src)
    }

    /// All input `PortRef`s that a given output port feeds.
    pub fn consumers_of(&self, src: PortRef) -> Vec<PortRef> {
        self.edges.iter().filter(|e| e.src == src).map(|e| e.dst).collect()
    }

    /// Direct predecessor NodeIds for a given node (via data edges).
    pub fn predecessors(&self, id: NodeId) -> Vec<NodeId> {
        let node = match self.nodes.get(&id) { Some(n) => n, None => return vec![] };
        let mut out: Vec<NodeId> = Vec::new();
        for (pi, _) in node.inputs.iter().enumerate() {
            let dst = PortRef { node: id, port: pi as u8 };
            if let Some(src) = self.source_of(dst) {
                if !out.contains(&src.node) { out.push(src.node); }
            }
        }
        out
    }

    /// Direct successor NodeIds for a given node (via data edges).
    pub fn successors(&self, id: NodeId) -> Vec<NodeId> {
        let node = match self.nodes.get(&id) { Some(n) => n, None => return vec![] };
        let mut out: Vec<NodeId> = Vec::new();
        for (pi, _) in node.outputs.iter().enumerate() {
            let src = PortRef { node: id, port: pi as u8 };
            for dst in self.consumers_of(src) {
                if !out.contains(&dst.node) { out.push(dst.node); }
            }
        }
        out
    }

    // ── Editor mutation methods ───────────────────────────────────────────────

    /// Add a word node using the built-in arity table; returns the new node's id.
    pub fn add_word_node(&mut self, word: &str) -> NodeId {
        let (n_pops, n_pushes) = word_arity(word);
        self.add_node(NodeKind::Word(Arc::from(word)), n_pops, n_pushes)
    }

    /// Remove a node and all incident edges; returns the removed edge ids.
    pub fn remove_node(&mut self, id: NodeId) -> Vec<EdgeId> {
        let removed: Vec<EdgeId> = self.edges.iter()
            .filter(|e| e.src.node == id || e.dst.node == id)
            .map(|e| e.id)
            .collect();
        self.edges.retain(|e| e.src.node != id && e.dst.node != id);
        self.nodes.remove(&id);
        self.node_order.retain(|&nid| nid != id);
        removed
    }

    /// Add an edge; returns Err if `dst` already has a source (no fan-in).
    pub fn add_edge(&mut self, src: PortRef, dst: PortRef) -> Result<EdgeId, ()> {
        if self.source_of(dst).is_some() { return Err(()); }
        Ok(self.connect(src, dst))
    }

    /// Remove a single edge by id; returns true if it existed.
    pub fn remove_edge(&mut self, id: EdgeId) -> bool {
        let before = self.edges.len();
        self.edges.retain(|e| e.id != id);
        self.edges.len() < before
    }

    /// All edge ids incident to a node (either end).
    pub fn edges_of(&self, nid: NodeId) -> Vec<EdgeId> {
        self.edges.iter()
            .filter(|e| e.src.node == nid || e.dst.node == nid)
            .map(|e| e.id)
            .collect()
    }

    /// Lower the graph to stax source text.
    ///
    /// After any editor mutation, call this and store the result as the
    /// canonical source string, then recompile.
    pub fn lower_to_source(&self) -> String {
        let ops = lower(self);
        ops_to_source_text(&ops)
    }
}

// ── Arity table ──────────────────────────────────────────────────────────────

/// Returns `(n_pops, n_pushes)` for a built-in word.
/// Unknown words default to `(1, 1)` (conservative passthrough assumption).
fn word_arity(word: &str) -> (usize, usize) {
    match word {
        // Stack manipulation
        "dup"    => (1, 2), "drop"   => (1, 0), "swap"  => (2, 2),
        "over"   => (2, 3), "rot"    => (3, 3), "nip"   => (2, 1),
        "tuck"   => (2, 3), "2dup"   => (2, 4), "2drop" => (2, 0),
        "2swap"  => (4, 4),

        // Binary → 1
        "+"  | "-"  | "*"   | "/"   | "%"   | "**"  |
        "==" | "!=" | "<"   | "<="  | ">"   | ">="  |
        "&"  | "|"  |
        "min" | "max" | "atan2" | "hypot" | "pow" |
        "to"  | "by" | "zip" | "lace" | "concat" | "++" |
        "cons" | "pair" | "tuple" |
        "shift" | "rot_left" | "rot_right" |
        "has" | "dot" | "set" |
        "lpf1" | "hpf1" | "lpf" | "lpf2" | "hpf" | "hpf2" |
        "pulse" | "lfnoise0" | "lfnoise1" | "dust" | "dust2" | "sah" |
        "ar" | "skip" | "take" |
        "upSmp" | "dwnSmp" |
        "grow" | "ngrow" | "byz" |
        "chebdist" | "farrow" |
        "logistic" | "coins" |
        "lpcanalz" | "goertzel" | "goertzelc" |
        "pan3" | "disperser" | "delayn" |
        "gaussian" | "kaiser" |
        "lindiv" | "expdiv" => (2, 1),

        // Unary → 1
        "neg"  | "abs"  | "recip" | "sqrt" | "exp"  | "log" |
        "log2" | "log10"| "sin"   | "cos"  | "tan"  | "asin"|
        "acos" | "atan" | "sinh"  | "cosh" | "tanh" |
        "ceil" | "floor"| "round" | "trunc"| "frac" |
        "sign" | "not"  | "sq"    | "cb"   | "sinc" |
        "size" | "#"    | "len"   |
        "first"| "last" | "butlast"| "rest"|
        "reverse"| "sort"| "grade"| "flatten"| "mirror"| "cyc"|
        "isReal"| "isStr"| "isSym"| "isStream"| "isSignal"|
        "isFun"| "isForm"| "isRef"| "isNil"|
        "keys" | "values"| "parent"| "local"|
        "ref"  | "get"  | "upd"   |
        "sinosc"| "saw" | "lfsaw" | "tri"  | "square"| "impulse"|
        "wnoise"| "white"| "pnoise"| "pink" | "brown"|
        "fadein"| "fadeout"| "hanenv"| "decay"| "decay2"|
        "fib"  | "primes"| "ever" | "2X"   |
        "normalize"| "peak"| "rms"| "dur"  |
        "mdct" | "imdct"| "hilbert"|
        "midihz"| "midinote"| "dbtamp"| "amptodb"|
        "hann" | "hamming"| "blackman"| "blackmanharris"|
        "nuttall"| "flattop"|
        "p"    | "trace"| "inspect"| "bench"|
        "softclip"| "hardclip"| "tanhsat"| "cubicsat"| "atansat"|
        "2ple" | "3ple" | "4ple" | "5ple"  | "6ple"| "7ple"| "8ple" => (1, 1),

        // Unary → 2
        "uncons" => (1, 2),

        // 3-arg → 1
        "clip" | "wrap" | "N" | "nby" |
        "rlpf" | "rhpf" | "lag" | "lag2" | "leakdc" |
        "svflp"| "svfhp"| "svfbp"| "svfnotch"|
        "firlp"| "firhp"| "combn"| "pluck"|
        "thiran"| "pan2"| "bal2"| "rot2"|
        "rands"| "irands"| "picks"|
        "skipWhile"| "keepWhile" => (3, 1),

        // 4-arg → 1
        "adsr" | "verb" | "lpcsynth" | "pvocstretch" | "pvocp" |
        "firbp"| "nby4" | "henon" => (4, 1),

        // 5-arg → 1
        "linlin"| "linexp"| "explin"| "bilin"| "biexp"|
        "vanderpol"| "cqt" => (5, 1),

        // 6-arg → 1
        "compressor"| "rossler" => (6, 1),

        // 7-arg → 1
        "lorenz"| "grain" => (7, 1),

        // Sources (0 pops, 1 push)
        "ord"  | "ordz" | "nat"  | "natz" |
        "sr"   | "nyq"  | "isr"  | "inyq" | "rps" |
        "rand" | "irand"| "muss" => (0, 1),

        // Sinks (consume, don't push)
        "play" => (1, 0),
        "stop" => (0, 0),
        "seed" => (1, 0),

        // Rank / each modifiers — pop the list/operand, produce nothing
        // (each-state is implicit in the interpreter; the graph carries the
        // ordering constraint via node_order).
        "@" | "@@" | "@@@" | "@1" | "@2" => (1, 0),

        // Default: conservative passthrough
        _ => (1, 1),
    }
}

// ── Lift: Vec<Op> → Graph ────────────────────────────────────────────────────

/// Lift a flat Op stream into a graph DAG.
///
/// A symbolic stack (`Vec<PortRef>`) tracks which node-output produced each
/// stack slot. Edges are added as values are consumed. The result faithfully
/// encodes the data-flow of the original program.
///
/// Stack underflows (from unknown-arity words where the stack is shallower than
/// expected) leave input ports unconnected. This doesn't break correctness of
/// `lower`, because lowering uses insertion order, not topology.
pub fn lift(ops: &[Op]) -> Graph {
    let mut g = Graph::new();

    // Symbolic stack: each slot is the PortRef that produced it.
    let mut stack: Vec<PortRef> = Vec::new();

    // Stack of (list-mark node id, stack-depth-at-mark) for [ ] construction.
    let mut mark_stack: Vec<(NodeId, usize)> = Vec::new();

    // Pending adverb to attach to the next Word node.
    let mut pending_adverb: Option<Adverb> = None;

    for op in ops {
        match op {
            // ── Literals ─────────────────────────────────────────────────────
            Op::Lit(v) => {
                let nid = g.add_node(NodeKind::Literal(v.clone()), 0, 1);
                stack.push(PortRef { node: nid, port: 0 });
            }

            // ── Words ─────────────────────────────────────────────────────────
            Op::Word(name) => {
                let (n_pops, n_pushes) = word_arity(name);
                let nid = g.add_node(NodeKind::Word(name.clone()), n_pops, n_pushes);

                // Attach pending adverb.
                if let Some(adv) = pending_adverb.take() {
                    if let Some(n) = g.node_mut(nid) { n.adverb = Some(adv); }
                }

                // Connect inputs from the stack (oldest first = port 0 = left operand).
                let drain_from = stack.len().saturating_sub(n_pops);
                let srcs: Vec<PortRef> = stack.drain(drain_from..).collect();
                for (pi, src) in srcs.into_iter().enumerate() {
                    g.connect(src, PortRef { node: nid, port: pi as u8 });
                }

                // Push outputs onto the symbolic stack.
                for i in 0..n_pushes {
                    stack.push(PortRef { node: nid, port: i as u8 });
                }
            }

            // ── Adverb prefix ─────────────────────────────────────────────────
            Op::Adverb(adv) => {
                // The parser emits Adverb immediately before the Word it modifies.
                // Store it and attach when the Word node is created.
                pending_adverb = Some(*adv);
            }

            // ── Each / rank-lift annotation ───────────────────────────────────
            // Op::Each is not emitted by the current parser (@ etc. come through
            // as regular Op::Word entries). Handle it defensively as a passthrough.
            Op::Each { .. } => {
                // No-op in the graph — the consuming Word node's rank annotations
                // are set by the editor in M5. Ignored during headless lift.
            }

            // ── Quote / Sym ──────────────────────────────────────────────────
            Op::Quote(name) => {
                let nid = g.add_node(NodeKind::Quote(name.clone()), 0, 1);
                stack.push(PortRef { node: nid, port: 0 });
            }

            Op::Sym(name) => {
                let nid = g.add_node(NodeKind::Sym(name.clone()), 0, 1);
                stack.push(PortRef { node: nid, port: 0 });
            }

            // ── Form access ──────────────────────────────────────────────────
            Op::FormGet(name) => {
                let nid = g.add_node(NodeKind::FormGet(name.clone()), 1, 1);
                if let Some(src) = stack.pop() {
                    g.connect(src, PortRef { node: nid, port: 0 });
                }
                stack.push(PortRef { node: nid, port: 0 });
            }

            Op::FormApply(name) => {
                let nid = g.add_node(NodeKind::FormApply(name.clone()), 1, 0);
                if let Some(src) = stack.pop() {
                    g.connect(src, PortRef { node: nid, port: 0 });
                }
            }

            // ── Bindings ─────────────────────────────────────────────────────
            Op::Bind(name) => {
                let nid = g.add_node(NodeKind::Bind(name.clone()), 1, 0);
                if let Some(src) = stack.pop() {
                    g.connect(src, PortRef { node: nid, port: 0 });
                }
            }

            Op::BindMany { names, list_mode } => {
                let n = names.len();
                let nid = g.add_node(
                    NodeKind::BindMany { names: names.clone(), list_mode: *list_mode },
                    n, 0,
                );
                let drain_from = stack.len().saturating_sub(n);
                let srcs: Vec<PortRef> = stack.drain(drain_from..).collect();
                for (pi, src) in srcs.into_iter().enumerate() {
                    g.connect(src, PortRef { node: nid, port: pi as u8 });
                }
            }

            // ── Call ─────────────────────────────────────────────────────────
            Op::Call => {
                // Conservative: pops the function, produces one output.
                // Real arity depends on the function and is unknown statically.
                let nid = g.add_node(NodeKind::Call, 1, 1);
                if let Some(src) = stack.pop() {
                    g.connect(src, PortRef { node: nid, port: 0 });
                }
                stack.push(PortRef { node: nid, port: 0 });
            }

            // ── List construction ────────────────────────────────────────────
            Op::ListMark => {
                let nid = g.add_node(NodeKind::ListMark, 0, 0);
                mark_stack.push((nid, stack.len()));
            }

            Op::MakeList { signal } => {
                let (_mark_nid, mark_depth) = mark_stack.pop().unwrap_or((NodeId(0), 0));
                let count = stack.len().saturating_sub(mark_depth);
                let nid = g.add_node(NodeKind::MakeList { signal: *signal }, count, 1);
                let drain_from = stack.len().saturating_sub(count);
                let srcs: Vec<PortRef> = stack.drain(drain_from..).collect();
                for (pi, src) in srcs.into_iter().enumerate() {
                    g.connect(src, PortRef { node: nid, port: pi as u8 });
                }
                stack.push(PortRef { node: nid, port: 0 });
            }

            // ── Form construction ────────────────────────────────────────────
            Op::MakeForm { keys, parent } => {
                let n = keys.len() + if *parent { 1 } else { 0 };
                let nid = g.add_node(
                    NodeKind::MakeForm { keys: keys.clone(), parent: *parent },
                    n, 1,
                );
                let drain_from = stack.len().saturating_sub(n);
                let srcs: Vec<PortRef> = stack.drain(drain_from..).collect();
                for (pi, src) in srcs.into_iter().enumerate() {
                    g.connect(src, PortRef { node: nid, port: pi as u8 });
                }
                stack.push(PortRef { node: nid, port: 0 });
            }

            // ── Function construction ─────────────────────────────────────────
            Op::MakeFun { params, body } => {
                // Closes over the current environment; doesn't consume stack values.
                let nid = g.add_node(
                    NodeKind::MakeFun { params: params.clone(), body: body.clone() },
                    0, 1,
                );
                stack.push(PortRef { node: nid, port: 0 });
            }
        }
    }

    g
}

// ── Lower: Graph → Vec<Op> ───────────────────────────────────────────────────

/// Lower a graph back to a flat Op stream by emitting nodes in insertion order.
///
/// Insertion order is the original parse order, so this is the identity for
/// graphs produced by `lift`. For graphs produced by the M5 editor (where the
/// user can reorder nodes), call `topo_sort` first and pass the sorted IDs to
/// `lower_ordered`.
pub fn lower(g: &Graph) -> Vec<Op> {
    lower_ordered(g, g.node_order.iter().copied())
}

/// Lower a graph to a flat Op stream using an explicit node ordering.
///
/// Used by the M5 editor after calling `topo_sort` to obtain an execution-valid
/// ordering of the user's visual layout.
pub fn lower_ordered(g: &Graph, order: impl Iterator<Item = NodeId>) -> Vec<Op> {
    let mut ops = Vec::new();
    for nid in order {
        let node = match g.node(nid) { Some(n) => n, None => continue };

        // Emit adverb just before the word it annotates.
        if let (NodeKind::Word(_), Some(adv)) = (&node.kind, node.adverb) {
            ops.push(Op::Adverb(adv));
        }

        let op = match &node.kind {
            NodeKind::Literal(v)  => Op::Lit(v.clone()),
            NodeKind::Word(w)     => Op::Word(w.clone()),
            NodeKind::Quote(w)    => Op::Quote(w.clone()),
            NodeKind::Sym(w)      => Op::Sym(w.clone()),
            NodeKind::FormGet(w)  => Op::FormGet(w.clone()),
            NodeKind::FormApply(w)=> Op::FormApply(w.clone()),
            NodeKind::Bind(w)     => Op::Bind(w.clone()),
            NodeKind::BindMany { names, list_mode } => {
                Op::BindMany { names: names.clone(), list_mode: *list_mode }
            }
            NodeKind::Call        => Op::Call,
            NodeKind::ListMark    => Op::ListMark,
            NodeKind::MakeList { signal } => Op::MakeList { signal: *signal },
            NodeKind::MakeForm { keys, parent } => {
                Op::MakeForm { keys: keys.clone(), parent: *parent }
            }
            NodeKind::MakeFun { params, body } => {
                Op::MakeFun { params: params.clone(), body: body.clone() }
            }
        };
        ops.push(op);
    }
    ops
}

// ── Topological sort (M5 utility) ────────────────────────────────────────────

/// Return a valid topological order of all nodes (all predecessors before
/// each node), using insertion order as a tie-breaker for determinism.
///
/// Used by the M5 editor when nodes have been repositioned visually. For
/// graphs produced by `lift` (no reordering), `lower` is simpler and faster.
pub fn topo_sort(g: &Graph) -> Vec<NodeId> {
    let order_idx: HashMap<NodeId, usize> = g.node_order
        .iter().enumerate().map(|(i, &id)| (id, i)).collect();

    // Kahn's algorithm with insertion-order tie-breaking.
    let mut in_deg: HashMap<NodeId, usize> = g.node_order.iter().map(|&id| (id, 0)).collect();
    for edge in g.edges() {
        *in_deg.entry(edge.dst.node).or_insert(0) += 1;
    }

    let mut ready: Vec<NodeId> = in_deg.iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&id, _)| id)
        .collect();
    ready.sort_by_key(|id| order_idx.get(id).copied().unwrap_or(usize::MAX));

    let mut result: Vec<NodeId> = Vec::with_capacity(g.node_count());

    while !ready.is_empty() {
        let nid = ready.remove(0);
        result.push(nid);

        for succ in g.successors(nid) {
            if let Some(d) = in_deg.get_mut(&succ) {
                *d = d.saturating_sub(1);
                if *d == 0 {
                    // Insert maintaining insertion-order sort.
                    let pos = ready.partition_point(|id| {
                        order_idx.get(id).copied().unwrap_or(usize::MAX)
                            < order_idx.get(&succ).copied().unwrap_or(usize::MAX)
                    });
                    ready.insert(pos, succ);
                }
            }
        }
    }

    // Any nodes not reached (disconnected singletons) — append in insertion order.
    for &id in &g.node_order {
        if !result.contains(&id) { result.push(id); }
    }

    result
}

// ── Source text emission ──────────────────────────────────────────────────────

fn adverb_suffix(adv: &Adverb) -> &'static str {
    match adv {
        Adverb::Reduce   => "/",
        Adverb::Scan     => "\\",
        Adverb::Pairwise => "^",
    }
}

/// Convert a flat `Vec<Op>` to canonical stax source text.
///
/// Handles all op types produced by `lower()`. Used by `Graph::lower_to_source`
/// and exposed pub so stax-editor can reuse it.
pub fn ops_to_source_text(ops: &[Op]) -> String {
    // Pre-scan: which ListMark indices are signal (#[) lists?
    let mut mark_stk: Vec<usize> = Vec::new();
    let mut sig_marks = std::collections::HashSet::new();
    for (i, op) in ops.iter().enumerate() {
        match op {
            Op::ListMark => mark_stk.push(i),
            Op::MakeList { signal } => {
                if let Some(m) = mark_stk.pop() {
                    if *signal { sig_marks.insert(m); }
                }
            }
            _ => {}
        }
    }

    let mut out = String::new();
    let mut pending_adv: Option<Adverb> = None;

    for (i, op) in ops.iter().enumerate() {
        match op {
            Op::Adverb(adv) => { pending_adv = Some(*adv); }
            _ => {
                if !out.is_empty() { out.push(' '); }
                match op {
                    Op::Lit(v) => {
                        match v {
                            Value::Real(x) => {
                                if *x == x.floor() && x.abs() < 1_000_000.0 {
                                    out.push_str(&format!("{}", *x as i64));
                                } else {
                                    out.push_str(&format!("{x}"));
                                }
                            }
                            Value::Str(s) => { out.push('"'); out.push_str(s); out.push('"'); }
                            Value::Sym(s) => { out.push('\''); out.push_str(s); }
                            Value::Nil    => out.push_str("nil"),
                            _             => out.push_str("0"),
                        }
                    }
                    Op::Word(w) => {
                        out.push_str(w);
                        if let Some(adv) = pending_adv.take() {
                            out.push_str(adverb_suffix(&adv));
                        }
                    }
                    Op::ListMark  => out.push_str(if sig_marks.contains(&i) { "#[" } else { "[" }),
                    Op::MakeList { .. } => out.push(']'),
                    Op::Bind(n) => { out.push_str("= "); out.push_str(n); }
                    Op::BindMany { names, list_mode } => {
                        let (l, r) = if *list_mode { ("[", "]") } else { ("(", ")") };
                        out.push_str("= "); out.push_str(l);
                        for (j, n) in names.iter().enumerate() {
                            if j > 0 { out.push(' '); }
                            out.push_str(n);
                        }
                        out.push_str(r);
                    }
                    Op::Quote(n)      => { out.push('`'); out.push_str(n); }
                    Op::Sym(n)        => { out.push('\''); out.push_str(n); }
                    Op::Call          => out.push('!'),
                    Op::FormGet(n)    => { out.push(','); out.push_str(n); }
                    Op::FormApply(n)  => { out.push('.'); out.push_str(n); }
                    Op::MakeForm { keys, .. } => {
                        out.push('{');
                        for (j, k) in keys.iter().enumerate() {
                            if j > 0 { out.push(' '); }
                            out.push(':'); out.push_str(k);
                        }
                        out.push('}');
                    }
                    Op::MakeFun { params, body } => {
                        out.push('\\');
                        let ps: Vec<&str> = params.iter().map(|s| s.as_ref()).collect();
                        if !ps.is_empty() { out.push_str(&ps.join(" ")); out.push(' '); }
                        out.push('[');
                        out.push_str(&ops_to_source_text(body));
                        out.push(']');
                    }
                    Op::Each { .. } => {}
                    Op::Adverb(_) => unreachable!("handled above"),
                }
                pending_adv = None;
            }
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use stax_eval::Interp;
    use stax_parser::parse;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn run_ops(ops: &[Op]) -> Vec<stax_core::Value> {
        let mut i = Interp::new();
        i.exec(ops).unwrap();
        i.stack
    }

    fn real(stack: &[stax_core::Value], idx: usize) -> f64 {
        stack[idx].as_real().expect("expected Real")
    }

    // Lift → lower → compare Op sequences structurally.
    fn roundtrip_ops(src: &str) -> (Vec<Op>, Vec<Op>) {
        let original = parse(src).unwrap();
        let graph    = lift(&original);
        let lowered  = lower(&graph);
        (original, lowered)
    }

    // ── Op identity ──────────────────────────────────────────────────────────

    #[test]
    fn identity_simple_arithmetic() {
        let (orig, low) = roundtrip_ops("2 3 +");
        assert_eq!(orig.len(), low.len(), "op count mismatch");
        // Both should be: Lit(2), Lit(3), Word("+")
        assert!(matches!(&low[0], Op::Lit(Value::Real(n)) if *n == 2.0));
        assert!(matches!(&low[1], Op::Lit(Value::Real(n)) if *n == 3.0));
        assert!(matches!(&low[2], Op::Word(w) if w.as_ref() == "+"));
    }

    #[test]
    fn identity_nested_arithmetic() {
        let (orig, low) = roundtrip_ops("2 3 + 4 *");
        assert_eq!(orig.len(), low.len());
        assert_eq!(orig.len(), 5);
    }

    #[test]
    fn identity_unary() {
        let (orig, low) = roundtrip_ops("5 neg");
        assert_eq!(orig.len(), low.len());
    }

    #[test]
    fn identity_dup_mul() {
        let (orig, low) = roundtrip_ops("7 dup *");
        assert_eq!(orig.len(), low.len());
    }

    #[test]
    fn identity_swap() {
        let (orig, low) = roundtrip_ops("3 5 swap -");
        assert_eq!(orig.len(), low.len());
    }

    #[test]
    fn identity_list() {
        let (orig, low) = roundtrip_ops("[1 2 3]");
        assert_eq!(orig.len(), low.len());
        // ListMark, Lit(1), Lit(2), Lit(3), MakeList
        assert_eq!(orig.len(), 5);
        assert!(matches!(low[0], Op::ListMark));
        assert!(matches!(low[4], Op::MakeList { signal: false }));
    }

    #[test]
    fn identity_signal_list() {
        let (orig, low) = roundtrip_ops("#[1.0 2.0]");
        assert_eq!(orig.len(), low.len());
        assert!(matches!(low[3], Op::MakeList { signal: true }));
    }

    #[test]
    fn identity_nested_list() {
        // [1 [2 3] 4]
        let (orig, low) = roundtrip_ops("[1 [2 3] 4]");
        assert_eq!(orig.len(), low.len());
    }

    #[test]
    fn identity_bind() {
        let (orig, low) = roundtrip_ops("42 = x");
        assert_eq!(orig.len(), low.len());
        assert!(matches!(&low[1], Op::Bind(n) if n.as_ref() == "x"));
    }

    #[test]
    fn identity_lambda() {
        let (orig, low) = roundtrip_ops("\\a b [a b +]");
        assert_eq!(orig.len(), low.len());
        assert!(matches!(&low[0], Op::MakeFun { params, body }
            if params.len() == 2 && body.len() == 3));
    }

    #[test]
    fn identity_adverb_reduce() {
        // +/ should round-trip as Adverb(Reduce) followed by Word("+")
        let (orig, low) = roundtrip_ops("[1 2 3 4] +/");
        assert_eq!(orig.len(), low.len());
        // Find the adverb in the lowered ops
        let has_adverb = low.iter().any(|op| matches!(op, Op::Adverb(Adverb::Reduce)));
        assert!(has_adverb, "expected Adverb(Reduce) in lowered ops");
    }

    #[test]
    fn identity_adverb_scan() {
        let (orig, low) = roundtrip_ops("[1 2 3] +\\");
        assert_eq!(orig.len(), low.len());
        let has_scan = low.iter().any(|op| matches!(op, Op::Adverb(Adverb::Scan)));
        assert!(has_scan);
    }

    #[test]
    fn identity_quote() {
        let (orig, low) = roundtrip_ops("`neg");
        assert_eq!(orig.len(), low.len());
        assert!(matches!(&low[0], Op::Quote(n) if n.as_ref() == "neg"));
    }

    #[test]
    fn identity_sym() {
        let (orig, low) = roundtrip_ops("'foo");
        assert_eq!(orig.len(), low.len());
        assert!(matches!(&low[0], Op::Sym(n) if n.as_ref() == "foo"));
    }

    #[test]
    fn identity_call() {
        let (orig, low) = roundtrip_ops("\\x [x x *] = sq  5 `sq !");
        assert_eq!(orig.len(), low.len());
    }

    // ── Graph structure ──────────────────────────────────────────────────────

    #[test]
    fn node_count_arithmetic() {
        let g = lift(&parse("2 3 +").unwrap());
        assert_eq!(g.node_count(), 3); // Lit(2), Lit(3), Word("+")
    }

    #[test]
    fn node_count_list() {
        // [1 2 3] → ListMark, Lit(1), Lit(2), Lit(3), MakeList = 5 nodes
        let g = lift(&parse("[1 2 3]").unwrap());
        assert_eq!(g.node_count(), 5);
    }

    #[test]
    fn edge_count_arithmetic() {
        // 2 3 + → edges: Lit(2)→+(in0), Lit(3)→+(in1) = 2 edges
        let g = lift(&parse("2 3 +").unwrap());
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn edge_count_list() {
        // [1 2 3] → edges: Lit(1)→](in0), Lit(2)→](in1), Lit(3)→](in2) = 3 edges
        // ListMark has 0 ports — no edges
        let g = lift(&parse("[1 2 3]").unwrap());
        assert_eq!(g.edge_count(), 3);
    }

    #[test]
    fn edge_count_chained() {
        // 2 3 + 4 * → edges: Lit(2)→+, Lit(3)→+, +(out0)→*, Lit(4)→* = 4
        let g = lift(&parse("2 3 + 4 *").unwrap());
        assert_eq!(g.edge_count(), 4);
    }

    #[test]
    fn source_nodes() {
        // In "2 3 +", Lit(2) and Lit(3) are sources; "+" is not.
        let g = lift(&parse("2 3 +").unwrap());
        let sources: Vec<_> = g.nodes_in_order().filter(|n| n.is_source()).collect();
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn sink_nodes() {
        // "2 3 +" — Word("+") pushes 1, so no sinks.
        let g = lift(&parse("2 3 +").unwrap());
        assert_eq!(g.nodes_in_order().filter(|n| n.is_sink()).count(), 0);

        // "42 = x" — Bind is a sink.
        let g2 = lift(&parse("42 = x").unwrap());
        assert_eq!(g2.nodes_in_order().filter(|n| n.is_sink()).count(), 1);
    }

    #[test]
    fn adverb_attached_to_word() {
        // "+/" → Word("+") node has adverb=Reduce
        let g = lift(&parse("[1 2 3] +/").unwrap());
        let plus = g.nodes_in_order()
            .find(|n| matches!(&n.kind, NodeKind::Word(w) if w.as_ref() == "+"))
            .expect("missing + node");
        assert_eq!(plus.adverb, Some(Adverb::Reduce));
    }

    #[test]
    fn predecessors_correct() {
        // "2 3 +" — + node has Lit(2) and Lit(3) as predecessors.
        let g = lift(&parse("2 3 +").unwrap());
        let plus_id = g.nodes_in_order()
            .find(|n| matches!(&n.kind, NodeKind::Word(w) if w.as_ref() == "+"))
            .map(|n| n.id)
            .unwrap();
        let preds = g.predecessors(plus_id);
        assert_eq!(preds.len(), 2);
    }

    #[test]
    fn successors_correct() {
        // "2 3 + 4 *" — + node has * as successor.
        let g = lift(&parse("2 3 + 4 *").unwrap());
        let plus_id = g.nodes_in_order()
            .find(|n| matches!(&n.kind, NodeKind::Word(w) if w.as_ref() == "+"))
            .map(|n| n.id)
            .unwrap();
        let succs = g.successors(plus_id);
        assert_eq!(succs.len(), 1);
        let succ = g.node(succs[0]).unwrap();
        assert!(matches!(&succ.kind, NodeKind::Word(w) if w.as_ref() == "*"));
    }

    // ── Topo sort ────────────────────────────────────────────────────────────

    #[test]
    fn topo_sort_arithmetic() {
        let g = lift(&parse("2 3 +").unwrap());
        let sorted = topo_sort(&g);
        assert_eq!(sorted.len(), 3);
        // Lit nodes must come before +
        let plus_pos = sorted.iter().position(|&id| {
            matches!(&g.node(id).unwrap().kind, NodeKind::Word(w) if w.as_ref() == "+")
        }).unwrap();
        for &id in &sorted[..plus_pos] {
            assert!(matches!(g.node(id).unwrap().kind, NodeKind::Literal(_)));
        }
    }

    #[test]
    fn topo_sort_chained() {
        let g = lift(&parse("2 3 + 4 *").unwrap());
        let sorted = topo_sort(&g);
        assert_eq!(sorted.len(), 5);
        let plus_pos = sorted.iter().position(|&id| {
            matches!(&g.node(id).unwrap().kind, NodeKind::Word(w) if w.as_ref() == "+")
        }).unwrap();
        let mul_pos = sorted.iter().position(|&id| {
            matches!(&g.node(id).unwrap().kind, NodeKind::Word(w) if w.as_ref() == "*")
        }).unwrap();
        assert!(plus_pos < mul_pos, "+ must precede *");
    }

    // ── Semantic round-trip ───────────────────────────────────────────────────

    #[test]
    fn roundtrip_eval_add() {
        let ops = parse("2 3 +").unwrap();
        let g   = lift(&ops);
        let low = lower(&g);
        let r1  = run_ops(&ops);
        let r2  = run_ops(&low);
        assert_eq!(r1.len(), r2.len());
        assert!((real(&r1, 0) - real(&r2, 0)).abs() < 1e-9);
        assert!((real(&r1, 0) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn roundtrip_eval_chained() {
        let ops = parse("2 3 + 4 *").unwrap();
        let g   = lift(&ops);
        let low = lower(&g);
        let r1  = run_ops(&ops);
        let r2  = run_ops(&low);
        assert!((real(&r1, 0) - real(&r2, 0)).abs() < 1e-9);
        assert!((real(&r1, 0) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn roundtrip_eval_dup() {
        // 7 dup * → 49
        let ops = parse("7 dup *").unwrap();
        let r1  = run_ops(&ops);
        let r2  = run_ops(&lower(&lift(&ops)));
        assert!((real(&r1, 0) - 49.0).abs() < 1e-9);
        assert!((real(&r1, 0) - real(&r2, 0)).abs() < 1e-9);
    }

    #[test]
    fn roundtrip_eval_swap() {
        // 3 5 swap - → 5 - 3 = 2
        let ops = parse("3 5 swap -").unwrap();
        let r1  = run_ops(&ops);
        let r2  = run_ops(&lower(&lift(&ops)));
        assert!((real(&r1, 0) - real(&r2, 0)).abs() < 1e-9);
        assert!((real(&r1, 0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn roundtrip_eval_list_reduce() {
        // [1 2 3 4 5] +/ → 15
        let ops = parse("[1 2 3 4 5] +/").unwrap();
        let r1  = run_ops(&ops);
        let r2  = run_ops(&lower(&lift(&ops)));
        assert!((real(&r1, 0) - 15.0).abs() < 1e-9);
        assert!((real(&r1, 0) - real(&r2, 0)).abs() < 1e-9);
    }

    #[test]
    fn roundtrip_eval_bind_use() {
        // 10 = x  x x * → 100
        let ops = parse("10 = x  x x *").unwrap();
        let r1  = run_ops(&ops);
        let r2  = run_ops(&lower(&lift(&ops)));
        assert!((real(&r1, 0) - 100.0).abs() < 1e-9);
        assert!((real(&r1, 0) - real(&r2, 0)).abs() < 1e-9);
    }

    #[test]
    fn roundtrip_eval_lambda_call() {
        // \a b [a b *] = mul  3 7 `mul ! → 21
        let ops = parse("\\a b [a b *] = mul  3 7 `mul !").unwrap();
        let r1  = run_ops(&ops);
        let r2  = run_ops(&lower(&lift(&ops)));
        assert!((real(&r1, 0) - 21.0).abs() < 1e-9);
        assert!((real(&r1, 0) - real(&r2, 0)).abs() < 1e-9);
    }

    #[test]
    fn roundtrip_eval_sym() {
        // 'hello — just pushes a symbol; both stacks should have the same Sym
        let ops = parse("'hello").unwrap();
        let r1  = run_ops(&ops);
        let r2  = run_ops(&lower(&lift(&ops)));
        assert_eq!(r1.len(), r2.len());
        match (&r1[0], &r2[0]) {
            (stax_core::Value::Sym(a), stax_core::Value::Sym(b)) => assert_eq!(a, b),
            _ => panic!("expected Sym"),
        }
    }

    #[test]
    fn roundtrip_eval_stream_n() {
        // 1 5 to 5 N → [1 2 3 4 5]
        let ops = parse("1 5 to 5 N").unwrap();
        let r1  = run_ops(&ops);
        let r2  = run_ops(&lower(&lift(&ops)));
        assert_eq!(r1.len(), r2.len());
    }

    // ── A6: mutation methods ──────────────────────────────────────────────────

    #[test]
    fn add_word_node_adds_to_order() {
        let mut g = Graph::new();
        let before = g.node_count();
        let nid = g.add_word_node("neg");
        assert_eq!(g.node_count(), before + 1);
        assert!(g.node(nid).is_some());
        // Node should appear in node_order
        let order = topo_sort(&g);
        assert!(order.contains(&nid));
    }

    #[test]
    fn remove_node_removes_incident_edges() {
        let mut g = lift(&parse("2 3 +").unwrap());
        assert_eq!(g.edge_count(), 2);
        // Find the + node
        let plus_id = g.nodes_in_order()
            .find(|n| matches!(&n.kind, NodeKind::Word(w) if w.as_ref() == "+"))
            .map(|n| n.id)
            .unwrap();
        let removed = g.remove_node(plus_id);
        assert_eq!(removed.len(), 2, "should have removed both incident edges");
        assert_eq!(g.edge_count(), 0);
        assert!(g.node(plus_id).is_none());
    }

    #[test]
    fn add_edge_rejects_duplicate_dst() {
        let mut g = Graph::new();
        let src1 = g.add_node(NodeKind::Literal(Value::Real(1.0)), 0, 1);
        let src2 = g.add_node(NodeKind::Literal(Value::Real(2.0)), 0, 1);
        let dst  = g.add_node(NodeKind::Word(Arc::from("+")), 2, 1);
        let p1 = PortRef { node: src1, port: 0 };
        let p2 = PortRef { node: src2, port: 0 };
        let in0 = PortRef { node: dst, port: 0 };
        // First edge ok
        assert!(g.add_edge(p1, in0).is_ok());
        // Second edge to same dst port rejected
        assert!(g.add_edge(p2, in0).is_err());
    }

    #[test]
    fn remove_edge_by_id() {
        let mut g = lift(&parse("2 3 +").unwrap());
        assert_eq!(g.edge_count(), 2);
        let eid = g.edges()[0].id;
        assert!(g.remove_edge(eid));
        assert_eq!(g.edge_count(), 1);
        assert!(!g.remove_edge(eid), "second removal should return false");
    }

    #[test]
    fn lower_to_source_produces_parseable_text() {
        let g = lift(&parse("2 3 +").unwrap());
        let src = g.lower_to_source();
        let reparsed = stax_parser::parse(&src);
        assert!(reparsed.is_ok(), "lower_to_source produced unparseable text: {:?}", src);
        // Source should contain the tokens
        assert!(src.contains('2') || src.contains("2"), "missing 2 in: {src}");
        assert!(src.contains('+'), "missing + in: {src}");
    }
}
