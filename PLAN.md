# stax — plan

A Rust reimplementation of [SAPF](https://github.com/lfnoise/sapf) (Sound As Pure Form, James McCartney) with a node-based editor that treats the graph and the text as two views over the same typed AST — plus a separate arrangement system, GPU-accelerated operators, and JIT-compiled signal paths that go beyond the original.

---

## Goals

1. **Language parity first.** The Rust interpreter should pass McCartney's `unit-tests.txt` before any UI work begins.
2. **Cross-platform runtime.** macOS, Windows, Linux, and WASM/AudioWorklet from a single codebase via `cpal` + `web-audio-api-rs`.
3. **Text ↔ graph isomorphism.** Edit in either representation; changes round-trip losslessly. The graph is a *view* over the AST, never a separate artifact.
4. **First-class functions in the graph.** Function-valued ports accept inline subgraphs (Bitwig Grid–style) or named library blocks. This is what sets it apart from Max/PD.
5. **APL-style density preserved.** `@`, `@1`/`@2`, `+/`, `+\` render as port/node modifiers (badges, toggles), not as separate nodes. Keeps SAPF's concision in the graph.
6. **Real upgrades over the original.** stax isn't just a port. An arrangement layer, GPU operators, JIT compilation, MIDI/OSC I/O, live visual feedback, and time-travel debugging land on top of the language core. See "Upgrades over the original" below.

## Non-goals (v1)

- Arrangement, mixing, and routing are v2 (M7 — see below).
- No attempt to replicate Max/PD's GUI widget ecosystem.
- No VST/AU plugin *host* in v1. Plugin *target* is M8.
- No multi-user editing.

---

## Architecture

Three layers, one typed IR between them. The arrangement layer (M7) sits above; GPU and JIT (M9, M10) sit beside the evaluator.

```
   ┌───────────────────────────────────────────────────────────────────┐
   │                        VIEW LAYER (M5 + M7)                       │
   │                                                                   │
   │   text · graph · fnport      arrangement · session · mixer        │
   │   └────── M5 ──────┘         └─────────── M7 ───────────┘         │
   │                       all 6 share one app shell                   │
   └─────────────────────────────────┬─────────────────────────────────┘
                                     ▼
         ┌─────────────────────────────────────────────────────┐
         │   typed AST / Op stream (stax-core)                 │
         │   the narrow waist — every view emits + consumes it │
         └───────────────────┬─────────────────────────────────┘
                             ▼
         ┌─────────────────────────────────────────────────────┐
         │   pull-based evaluator (stax-eval)                  │
         │   stack machine + lazy streams + signals            │
         │                                                     │
         │     ┌───────────────────┐    ┌─────────────────┐    │
         │     │ stax-jit [M10]    │    │ stax-gpu [M9]   │    │
         │     │ cranelift signal  │    │ wgpu operators  │    │
         │     └───────────────────┘    └─────────────────┘    │
         └───────────────────┬─────────────────────────────────┘
                             ▼
         ┌─────────────────────────────────────────────────────┐
         │   audio + MIDI/OSC runtime (stax-audio, stax-io)    │
         │   cpal native / AudioWorklet WASM / midir / rosc    │
         └─────────────────────────────────────────────────────┘
```

**Why this split:** every view emits the same `Vec<Op>`. This is how Max/Gen works and it's what makes round-tripping tractable — diff the Op stream, not the visual layout. The arrangement layer is another `Op`-emitter; GPU and JIT are alternate *backends* for `Op` subgraphs matching their eligibility rules.

### Six views, one source

The product surfaces as six views over a single `night_drive.stax` file. The mockups in `design/` are the spec. All six render in the same egui app shell — switching tabs never reloads the interpreter, never re-parses, never loses transport state. They share one `Interp`, one `Op` stream, one set of design tokens, and one inspector contract on the right side.

| # | view | what it shows | implementation milestone |
|---|---|---|---|
| 01 | **graph** | node canvas, ports, wires, rank badges, adverb toggles | M5 |
| 02 | **fnport** | function-valued port detail (drill-in / named / nested) | M5 |
| 03 | **arrangement** | linear timeline, clips, automation lanes | M7 |
| 04 | **session** | clip-launcher grid, scenes, queued/playing states | M7 |
| 05 | **mixer** | channel strips with stax inserts and sends | M7 |
| 06 | **text** | canonical IDE source — line numbers, syntax highlight, stack-at-cursor | M5 |

Cross-view "reveal in" is a single shared service: any view holding a selection (a node, a clip, an insert, a code range) can request another view to scroll/highlight the corresponding region. Implemented as a `SourceRange → ViewId → Selection` resolver living in `stax-editor`. This is the technical core of the "edit anywhere · all views are one source" tagline.

### Shared editor services

These are built once in `stax-editor` and consumed by every view, so no view re-implements them:

- **App shell** — top bar, tabs, bottom status bar, 240 px right inspector. Layout tokens live in `editor::shell`.
- **Syntax highlighter** — lexes `&str` → token stream → semantic spans. Used by the text view, by clip bodies in arrangement / session, by insert labels in mixer, and by inspector "expression" panes everywhere.
- **Expression renderer** — given a `&[Op]`, renders the canonical text form with the same highlighting. The single source of truth for "how do we display stax code" wherever it appears.
- **REPL pane** — embeddable in any view, shares the parent's `Interp`. Always-on in editor, text, and arrangement; collapsible in mixer/session.
- **Live scope / spectrum widget** — drop-in for any signal-typed value. Inline in graph sinks, in arrangement clip bodies, in mixer meters.
- **Reveal-in router** — the cross-view jump service.


### Pull-based evaluation

SAPF's semantics are pull-based and lazy. Walking from sinks (`play`, `plot`, `record`) backward, each node's output is either:

- `Box<dyn StreamIter>` — lazy value sequence (one pull → one `Value`)
- `Box<dyn SignalInstance>` — audio-rate buffer producer (one pull → `N` samples)

Rust's `Iterator` trait is almost spookily well-matched to SAPF's `Stream`. Most of the stream primitives (`to`, `take`, `drop`, `cycle`, `zip`, `map`) are one-liners over the iterator adapter library.

### Value model

Ref-counted immutable values. Everything except `Ref` is shared-immutable and cheaply cloneable.

```rust
pub enum Value {
    Real(f64),
    Str(Arc<str>),
    Sym(Arc<str>),            // 'sin
    Stream(Arc<dyn Stream>),  // lazy value seq — produces fresh iterators
    Signal(Arc<dyn Signal>),  // audio-rate — produces fresh instances
    Form(Arc<Form>),
    Fun(Arc<Function>),
    Ref(Arc<RwLock<Value>>),  // only mutable variant
    Nil,
}
```

`Stream` and `Signal` are *descriptions*, not running iterators. You ask them for an iterator/instance. This preserves reusability — `play`ing a signal twice creates two independent instances, which is what SAPF already does implicitly.

---

## Crate layout

```
stax/
├── Cargo.toml             # workspace
├── PLAN.md                # this file
├── crates/
│   ├── stax-core/         # Value, Stream, Signal, Form, Function, Op, errors
│   ├── stax-parser/       # postfix text → Vec<Op>
│   ├── stax-eval/         # Interp (stack machine), built-in words
│   ├── stax-dsp/          # sinosc, saw, lfsaw, combn, ... ; SIMD buffers
│   ├── stax-audio/        # cpal native runtime + wasm AudioWorklet bridge
│   ├── stax-io/           # [M3] MIDI (midir), OSC (rosc) — first-class I/O
│   ├── stax-graph/        # [M4] graph ↔ Op stream round-trip
│   ├── stax-editor/       # [M5] egui node editor + live-viz REPL + time-travel
│   ├── stax-arrange/      # [M7] transport, clips, patterns, automation
│   ├── stax-plugin/       # [M8] nih-plug VST3/CLAP target
│   ├── stax-gpu/          # [M9] wgpu operators — additive, granular, FDTD, conv
│   └── stax-jit/          # [M10] cranelift JIT for pure signal subgraphs
├── examples/
│   └── repl.rs            # stax-eval + stax-audio CLI REPL
└── design/                # static HTML mockups — visual vocabulary for M5
    ├── 01-editor.html     # main node-graph editor
    ├── 02-fnport.html     # function-valued port detail (drill-in / named / nested)
    ├── 03-arrangement.html # M7 — linear timeline
    ├── 04-session.html    # M7 — clip-launcher grid
    ├── 05-mixer.html      # channel strips with stax inserts/sends
    └── 06-text.html       # IDE / text view — canonical source, all-views-are-one
```

Crates marked with milestones are deferred — scaffolding lives in the workspace but they aren't populated until the language core is solid.

---

## v1 milestones (M0–M6)

### M0 — scaffold (current state)
Workspace builds. `Value` enum, `Stream`/`Signal` traits, `Op` enum, empty `Interp` compile and have the right shape. No real behavior yet.

### M1 — minimal interpreter
- Parser for: numbers (with suffixes `k`, `M`, `m`, `u`, `pi`, fractions), strings, words, `[...]` lists, `{...}` forms, backquote/quote/comma/dot/equals, `\args [body]` functions
- `Op` stream: `Lit`, `Word`, `Quote`, `Sym`, `Bind`, `Call`, `MakeList`, `MakeForm`, `MakeFun`, `Each(n)`
- Built-in words: arithmetic (`+ - * /`), list constructors (`to`, `ord`, `nat`), stack ops (`dup drop swap over`)
- Passes ~30% of `unit-tests.txt`

### M2 — auto-mapping and adverbs
- Rank-polymorphic dispatch: binary ops auto-map over streams/signals
- `@`, `@1`, `@2` each operators (ordered for outer products)
- `+/`, `+\`, `+^` reduce/scan/pairwise adverbs on all binary math ops
- **Deterministic seeds** — every stochastic primitive takes a seed; session has a master seed; reproducible composition
- Passes ~70% of `unit-tests.txt`

### M3 — audio runtime + MIDI/OSC
- `stax-dsp`: `sinosc`, `saw`, `lfsaw`, `combn`, `pluck`, white/pink noise, envelopes
- Fixed-block audio-rate processing (64 or 128 samples)
- SIMD via `wide` or nightly `std::simd`; fall back to scalar on WASM
- `play` / `stop` words backed by `cpal` on native
- Spectrogram rendering via `realfft` → PNG
- **`stax-io`: MIDI in/out via `midir`, OSC via `rosc`** — first-class, not an afterthought. This is what lets stax talk to hardware, DAWs, Ableton Link, TidalCycles, etc.
- Passes 95%+ of `unit-tests.txt`. Can actually make sound and send/receive control.

### M3.5 — DSP deferred items (post-M4, before M5)

These were requested but deferred from M3 due to complexity. Implement as `stax-dsp` structs + `stax-eval` words with tests:

- **Synchrosqueezing Transform (SST/SSWT)**: Requires CWT (Morlet wavelet) or STFT, then instantaneous frequency estimation via phase derivative, then energy reassignment onto the TF plane. Suggested API: `signal scales sst → VecSignal` (reassigned time-frequency magnitude). Depends on complex wavelet convolution per scale.

### M4 — graph IR + round-trip
- `stax-graph`: `NodeId`, `PortId`, `Edge`, typed ports
- `Op` stream ↔ graph conversion, both directions, stable
- Headless tests: parse text → compile to Ops → lift to graph → lower to Ops → eval → bit-identical output

### M5 — editor surface: text + graph + fnport

Three views in one milestone because they share the egui app shell, the same `Interp`, the syntax highlighter, the expression renderer, the REPL pane, and the live-scope widget. Build the shared services first, then the three views in the order below — each new view validates more of the shared infrastructure.

**Order matters here.** Don't start with the graph editor; it's the most complex and the hardest to debug. Start with text — it's nearly free once the parser and Interp work, and it shakes out every shared service before the graph view depends on them.

**Step 1 — App shell and shared services.** Implement once, reused everywhere.
- `editor::shell` — top bar, tabs, 240 px right inspector, 28 px status bar, layout tokens
- `editor::syntax` — lex `&str` to semantic spans; the color scheme is the design tokens (`port-fun` for `\`, `=`; `port-signal` for operators; `port-stream` for strings; etc.)
- `editor::expr` — render `&[Op]` to highlighted text; the canonical "show me this code" function
- `editor::repl` — embeddable REPL pane sharing the host view's `Interp`
- `editor::scope` — drop-in scope/spectrum/meter widget for any `Signal`
- `editor::reveal` — the cross-view "select this range, jump to that view" router
- Custom canvas on `egui` primitives (not `egui_node_graph` — see design decisions)

**Step 2 — Text view (06).** Simplest of the three; tests every shared service.
- Line numbers, gutter (live-execution dots, modification marks), active-line highlight
- Syntax highlighter consumed by `editor::syntax`
- Outline panel — every `=` binding with line number; click to scroll
- Hover docs popup over built-ins (signature + description from the prelude)
- Inline error squigglies + popup hints with "did you mean" suggestions
- **Stack-at-cursor panel** in the right inspector — the unique stax-shaped IDE feature; for a stack-based language, knowing your stack at any cursor position is everything
- Selection-action chip floating above selection: evaluate, reveal in graph, reveal in mixer
- Diagnostics panel in the file tree
- File tree, project explorer
- REPL embedded at the bottom of the right side

**Step 3 — Graph view (01).** Depends on M4 (graph IR) being solid.
- Custom canvas — pan/zoom, dotted-grid background, hit-testing for nodes/ports/wires
- Typed ports color-coded by `Value` kind; wires color-match the source port
- Rank badges on input ports — click to cycle `0 / @ / @1 / @2 / @@`
- Adverb dropdowns on binary-op nodes — apply / reduce / scan / pairwise
- Library sidebar with categorized words + user-defined function chips
- Live REPL pane sharing the same `Interp`
- **Live visual feedback** — every signal expression shows an inline scope + spectrum + level meter via `editor::scope`. Click to expand.

**Step 4 — Fn-port subgraph drill-in (02).** Not a separate view — a node-level interaction pattern that lives in the graph view.
- Three states for each function-valued port: empty / inline subgraph / named binding
- Inline mode — recursive node body rendering with input/output stubs as boundary markers
- Named mode — a chip from the user library is bound; chip → port wire is violet
- Promote-to-named, unbind, open-in-tab toolbar
- Depth indicator in the subgraph zone corner
- Same `editor::expr` renderer used to populate the text-equivalent panel

**Step 5 — Cross-cutting features.**
- **Time-travel debugging** — snapshot `(stack, env)` after each Op into a ring buffer. Scrub backward through execution via the bottom bar's scrubber. Free because `Value` is immutable. No other live-coding language does this well — real differentiator.
- **Reveal-in router wired up everywhere** — text selection → graph nodes; graph node → text range; either → mixer insert if the expression is bound to one
- **Saved layout** — which views were open, splits, scroll positions, persisted per-project

### M6 — WASM + AudioWorklet + browser sharing
- `wasm-pack` build of `stax-eval` + `stax-dsp`
- AudioWorklet node that runs the interpreter on the audio thread
- Browser-hosted editor. Sidesteps macOS-only problem entirely.
- **Shareable URLs** — patches serialize into the URL fragment; open a link, hear the patch. Nearly free given the WASM build.

---

## Upgrades over the original (M7+)

This is where stax justifies its existence beyond "SAPF but cross-platform." Each of these is a standalone value proposition; they don't need to ship together.

### M7 — stax-arrange + arrangement/session/mixer views

The single biggest product-shaped gap in SAPF. An upper layer analogous to how a Max patcher sits above Gen, and three coordinated views surfacing it in the editor app shell.

**Underlying model (one crate, three views):**

- Session with transport: bpm, meter, position, tap tempo, Ableton Link
- Multiple tracks, each holding clips
- Clips contain stax expressions; triggering instantiates the signal, releasing stops it with a tail
- **Patterns as stax streams** driving clip triggers — `"x . x . x x . x"` or full generative streams
- **Automation lanes that are themselves stax expressions** — `[0 1] 4bars lfsaw` as a filter cutoff over 4 bars
- Routing: sends, buses, master
- MIDI I/O so it syncs to hardware and DAWs (reuses `stax-io` from M3)
- **Hot-reload with bar-boundary crossfade** — edit, recompile, crossfade on next bar. Tidal/Sonic Pi trick.

The elegant part: stax streams already ARE event sequences. The arrangement layer doesn't need a new model, just a *time base* (a stream yielding at bar/beat boundaries) and a trigger protocol for signals. A clip is a `(Stream<Trigger>, Fn(Trigger) -> Signal)` pair. Automation is a `Signal` at control rate. Pure-stax all the way down.

**View order — same logic as M5: simplest first.**

**Step 1 — `stax-arrange` core model.**
- `Transport`, `Track`, `Clip`, `AutomationLane`, `Send`, `Bus` types
- Clip serialization to stax expressions (each project saves as a single `.stax` file — text view round-trips)
- Scheduler: `Stream<Bar>` time base, queued vs playing clip states
- MIDI clock + Ableton Link integration via `stax-io`

**Step 2 — Mixer view (05).** Simplest visible view; just channel strips with insert chains.
- Channel strip: header, inserts (each is a stax expression with bypass + reorder), sends, pan, fader, meter, MSR
- Returns and master with their own visual treatment (paper-2 / surface backgrounds, 2 px ink rules)
- Inspector on the right shows the selected insert (its expression, params, breadcrumb)
- Bottom bar shows total signal-graph latency
- All inserts compile to one signal graph; the mixer is a *layout* of the routing, not a separate engine

**Step 3 — Arrangement view (03).** Linear timeline; the conventional DAW surface.
- Track headers on the left, ruler at top, lanes filling the rest
- Clips render with one of: pattern dots / code text / mini graph thumbnail / waveform fill — selectable per cell
- Automation sub-lane below each track, showing the curve + the expression label that generates it
- Loop region overlay, playhead, bar/beat snapping
- Master lane at the bottom with summed waveform preview

**Step 4 — Session view (04).** Clip-launcher grid; mostly the arrangement model rearranged.
- 5×N grid (track columns × scene rows)
- Three explicit cell states: empty (dotted), stopped (solid border), playing (warm fill on name strip), queued (dashed gold border, pulsing)
- Scene rail on the left with active/queued indicators and per-scene launch buttons
- Per-track stop button + volume readout at the foot of each column
- Stop-all in the bottom-left corner
- MIDI note assignment per clip for hardware launching (mocked in inspector)

**Step 5 — Cross-view sync.**
- Mixer ↔ arrangement: editing a track volume in either reflects in the other
- Session ↔ arrangement: triggering a clip in session creates an entry in the arrangement at the playhead
- All three ↔ text view via the reveal-in router from M5: select an insert, jump to its definition in `patch.stax`

### M8 — stax-plugin (VST3/CLAP target)

`nih-plug` wrapper exposing a compiled stax patch as VST3/CLAP. Load it in Ableton, Bitwig, Reaper, FL. Parameters exposed as `Ref` bindings in the patch.

Relevant to existing JUCE plugin work: the JUCE port can consume the same compiled `Op` stream, so stax becomes a cross-renderer DSP spec.

### M9 — stax-gpu (GPU operators)

Not transparent offload — **explicit operators** for workloads where GPU genuinely wins:

- **Additive synthesis** with hundreds/thousands of partials (`additive-gpu N`)
- **Granular clouds** with thousands of simultaneous grains
- **2D/3D physical models** — FDTD plates, membranes, waveguide meshes
- **Long convolution reverbs** (partitioned convolution)
- **Large wavetable banks** with dense interpolation

Stack: **wgpu** for cross-platform (Vulkan / Metal / DX12 / WebGPU from one API), WGSL compute shaders. Ships to browser for free since WebGPU is wgpu's web backend. Feature-flagged so it compiles out on platforms without wgpu.

Loss cases to document: single oscillators, IIR filters, any per-sample feedback, low voice counts. CPU↔GPU round-trip adds 2–4 blocks of buffering latency, so these are for synthesis, not real-time DI processing.

### M10 — stax-jit (JIT-compiled signal paths)

The matching *inner* layer to arrangement's *outer* one — the true Gen analog.

Any stax expression touching only `Signal` + `Real` (no streams, forms, refs) is a pure DSP kernel and can be compiled to native via **cranelift**. Same semantics as the interpreter, order-of-magnitude speedup on complex patches. Kicks in only on hot, stable signal graphs — anything `play`-ed, basically. Interpreter stays the default. IR is the existing `Op` stream with signal-typed dataflow extracted.

### Smaller wins (continuous, woven in)

- **Polyphonic voice allocation** — `voices 32 [...]` as a combinator managing allocation and stealing (M3 or later, library word)
- **Rust-quality error diagnostics** — source spans, "did you mean", underlined squigglies — standard throughout
- **Spectral primitives** — phase vocoder, constant-Q, pitch-shift (post-M9, some GPU-accelerated)

### Deferred / speculative

- **Plugin host** — load VST3/CLAP as stax effects via `clack`. Post-M10.
- **CRDT collaborative editing** — real-time co-editing of patches. Speculative.
- **Gradual typing in the graph** — type annotations become compile-time checks. Speculative.

---

## Design decisions (resolved)

### Language / runtime

**Rank lifting as modifier, not node.** If `[1 2] @ [10 20] +` became three graph nodes, the graph would be three times the size of the equivalent stax text. The `@` stays on the port as a small badge (click to cycle `0 / @ / @1 / @2 / @@`). Same for `+/` `+\` `+^` — a dropdown on the `+` node, not separate nodes.

**Function-valued ports — two modes.**
1. *Inline subgraph* — double-click the port, a mini-graph opens whose input stubs match the function's named arguments. Good for lambdas.
2. *Named block library* — functions defined with `= name` appear in a sidebar. Drag one onto a function port to bind. Good for the prelude and user-defined words.

Prototype inline; promote to named when a pattern recurs. Same codepath underneath — both produce a `Function` `Value`.

**No separate control-rate.** SAPF doesn't distinguish control from audio rate the way SuperCollider does. Don't add that back. Signals are signals; if you want slow, use a slow signal.

**Memoization deferred.** SAPF's lazy lists appear to be memoized — iterate twice, compute once. For M1 streams produce fresh iterators each time, which is correct but potentially wasteful. Add memoization in M2 once profiling points to it.

**Concurrency.** `Value` is `Send + Sync` because everything inside is immutable or locked. This is the single biggest win Rust gives us over the C++ original — parallel stream evaluation is safe by construction, no refcount races.

### Open questions, now resolved

**GC vs cycles — `Arc` only, document the footgun.** SAPF is nearly pure; `Ref` is an escape hatch, not an idiom. Real programs don't build `Ref` cycles. Adding tracing GC would permeate every `Value`-touching API and introduce pause-time unpredictability that's actively bad on the audio thread. If cycle leaks show up in real use, drop in `bacon-rajan-cc` — a cycle collector that plugs in alongside `Arc`. Don't pre-pay for a problem that may not exist.

**Parser — hand-roll.** The postfix grammar compiles tokens to a flat `Op` stream, not a tree AST, so combinator libraries (chumsky, winnow) don't buy much. Hand-rolled errors will be better than any library's, which matters for a live-coding REPL, and the WASM bundle stays smaller. Roughly 300 lines total: a `Token` enum with source spans, a `Lexer` as iterator-over-tokens, a `Compiler` that consumes tokens and emits `Vec<Op>`. Number suffixes and fractions live in the lexer via explicit character classes.

**Editor — custom canvas on egui primitives.** `egui_node_graph` is a fine reference but every differentiating feature — rank badges, inline subgraph drill-in, adverb toggles, text/graph split view — requires stepping outside its template system. Its `NodeData`/`DataType` abstractions would fight more than help. Build directly on `egui` + `egui_extras`. Study `egui_node_graph`'s source for hit-testing, connection snapping, port compatibility conventions, but don't depend on it. The editor is the product's identity — own it end-to-end.

**License — clean-room, dual MIT/Apache-2.0.** Do not read `src/*.cpp`. Work from `README.txt`, `unit-tests.txt`, and `sapf-examples.txt`. Write the stax prelude from scratch rather than copying `sapf-prelude.txt` (which is SAPF code — reimplementing it is cleaner than redistributing it). Keeps us compatible with Rust ecosystem defaults, avoids copyleft propagation, and preserves the plugin-target option since most VST3/CLAP SDK licenses are GPL-incompatible. Credit McCartney prominently regardless.

---

## Credits

stax is a clean-room Rust reimplementation of [SAPF (Sound As Pure Form)](https://github.com/lfnoise/sapf) by James McCartney, who is also the author of SuperCollider. The language semantics, the prelude philosophy, the example corpus, and the core insight of combining APL-style rank polymorphism with lazy concatenative programming for audio are entirely McCartney's work.

stax's contribution is the implementation (Rust, cross-platform, WASM), the node editor, the arrangement layer (M7), GPU operators (M9), JIT compilation (M10), and the live-debugging and time-travel affordances in the editor. The design of these sits on top of McCartney's language and makes sense only because his design is already good.

McCartney deserves the credit for everything that makes stax interesting at the language level. Any bugs or questionable design choices in stax are ours.

---

## First-week concrete steps

1. `cargo check --workspace` — M0 green.
2. Implement the parser for numbers and words only. Get `1 2 +` → `Op::Lit(Real(1)), Op::Lit(Real(2)), Op::Word("+")` → eval → `3` on the stack.
3. Implement `to`, `ord`, `take`. Get `0 ord to` producing a lazy stream, and `0 ord to 5 take` returning the first five sublists.
4. Wire `cpal` with a hard-coded 440 Hz sine (already done in `stax-smoke`). Once that plays, replace it with `440 0 sinosc .3 * play`.
5. Only then start thinking about the graph.
