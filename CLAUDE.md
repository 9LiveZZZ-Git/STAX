# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**stax** is a Rust reimplementation of SAPF (Sound As Pure Form, by James McCartney) — a stack-based concatenative audio programming language with a planned node-based visual editor. The system treats text and graph as two isomorphic views over the same typed AST/IR. See `PLAN.md` for the full 10-milestone roadmap and `design/` for static HTML mockups of the six planned editor views.

## Commands

```bash
cargo check --workspace          # verify the full workspace compiles
cargo run --bin stax-repl        # interactive arithmetic REPL
cargo run --bin stax-smoke       # 440 Hz sine smoke test (1 second of audio)
cargo test --workspace           # run all unit tests
cargo test -p stax-eval          # run tests for a single crate
cargo clippy --workspace         # lint
```

## Architecture

### The Narrow Waist: `Op`

`Vec<Op>` is the IR that everything flows through. Both the text parser and the graph editor emit `Vec<Op>`; the interpreter consumes it. This single IR is what makes lossless text ↔ graph round-tripping possible (planned M4+).

### Three Layers

**View layer (M5+)** — Six views (text, graph, function-port detail, arrangement, session, mixer) all share one `Interp`, one `Op` stream, one app shell. Views are lenses on the same data, not separate systems.

**Computation core (M0–M10)** — Stack machine interpreter in `stax-eval`. Pull-based: evaluation walks backward from sinks (`play`, `plot`, `record`) using lazy iterators. `Stream` and `Signal` are *descriptions*, not running state — ask them for an instance to preserve reusability.

**Runtime layer** — `cpal` native audio I/O, with a planned AudioWorklet WASM bridge (M6). Audio processed in fixed 64/128-sample blocks.

### Crate Map

| Crate | Role |
|---|---|
| `stax-core` | `Value`, `Stream`, `Signal`, `Form`, `Function`, `Op`, error types |
| `stax-parser` | text → `Vec<Op>` |
| `stax-eval` | `Interp` stack machine + built-in words |
| `stax-dsp` | oscillators, filters (M0: SinOsc only) |
| `stax-audio` | cpal runtime + WASM stub |
| `stax-graph` | graph IR + round-trip (M4, scaffolded) |
| `stax-editor` | egui node editor (M5, scaffolded) |
| `stax-arrange` | transport, clips, automation (M7, scaffolded) |

### Value Model

All values are `Arc<T>` or `Copy` — immutable by default. The only mutable value is `Ref` (backed by `RwLock`). This enables parallel evaluation and free time-travel debugging. `ValueKind` drives port colors in the graph editor and compile-time type checking.

### Language Semantics

stax is postfix/concatenative: `2 3 +` → 5. Key adverbs:
- `@` (each / map), `@1`/`@2` (outer product) — rank-lifting
- `+/` (reduce), `+\` (scan)

Rank-lifting adverbs are rendered as port/node *badges* in the graph view, not as separate nodes.

## Design System

The `design/` folder contains six self-contained HTML mockups that are the **authoritative visual spec** for the editor. Open in a browser, no build step. Any UI work must converge on these mockups. The `design/README.md` has the full rationale.

### Direction: Refined Brutalism
Functional honesty of Pure Data + visual care of a well-drafted technical document. Not a 90s X11 app, not a skeuomorphic DAW.

### Five Tenets
1. **Color carries meaning, not decoration.** Port-type hues appear nowhere else in the UI. `--warm` (terracotta) marks active/playing/selected — and only those states.
2. **No skeuomorphism.** No rounded corners, no shadows, no gradients, no glass, no faux-3D knobs. A knob is a line on a circle.
3. **Single typeface.** JetBrains Mono throughout — weight and tracking carry typographic hierarchy.
4. **Information density without noise.** Dotted hairlines (`--rule-2`) for sub-divisions, solid for primary structure.
5. **PD honesty.** Empty things look empty (dotted ghost outlines). Expressions shown literally everywhere.

### Color Tokens

```
--paper      #f4f1ea   background
--paper-2    #efebe1   subtle surface tint
--surface    #ebe7dd   panel surface (master strip, inspector)
--rule       #d4cfc3   primary divider
--rule-2     #bcb5a4   secondary divider, often dotted

--ink        #1a1a1a   primary text, strong borders
--ink-2      #6b6558   secondary text, labels
--ink-3      #9a9383   tertiary text, disabled

--warm       #c94820   terracotta — ONLY: active / playing / selected / errors
--cool       #2d5a4a   deep teal — meters, Ableton Link
--queued     #d4a017   muted gold — queued state (session view only)

Port colors (appear NOWHERE else):
--port-real    #1a1a1a   scalars         → filled black square
--port-signal  #c94820   audio-rate      → filled terracotta square
--port-stream  #2d5a4a   lazy sequences  → dashed teal border, transparent bg
--port-fun     #6b4e8a   functions       → 1.5px violet border, transparent bg
--port-form    #8a6b2a   dictionaries    → filled brown square

Track colors (muted, used only in arrangement/session/mixer):
--t-kick  #8a4a4a   --t-bass  #2d5a4a   --t-pad  #6b4e8a
--t-lead  #c94820   --t-fx    #8a6b2a   --t-rev/del  #4a6b8a
```

### App Shell Layout (all DAW views)

```
grid-template-rows: 40px  28px  1fr   38px
                    header tabs  main  botbar
max-width: 1600px, centered, border-left/right 1px --rule
```

- **Header** (40px): `brand | menu | spacer | audio stat | transport`
- **Tabs** (28px): `tab tab ... | spacer | meta`; active tab: 1.5px `--warm` underline, flush to bottom
- **Inspector** (always 240px right column): `h3` headers are `10px / 700 / 0.18em uppercase --ink-3 / dotted underline`
- **Bottom bar** (38px): level meters, mode indicators, status

### View-Specific Layouts

| View | Main grid | Left column | Content |
|---|---|---|---|
| graph | `200px \| canvas \| 240px` + 120px REPL + 34px time-travel | library | node canvas (dot grid bg) |
| arrangement | `188px \| timeline \| 240px` | track headers + ruler spacer | ruler + scrollable lanes |
| session | `188px \| clip grid \| 240px` | scene rail | track columns × scene rows |
| mixer | `1fr \| 240px` | channel strips (124px each, master 168px) | inspector |
| text | `200px \| editor \| 280px` | files + outline + diagnostics | code + side panels (stack, inspector, REPL) |

### Node & Wire Rendering

- Node: `1px solid --ink`, `background: --paper`, no border-radius
- Selected node: `border-color: --warm`, `box-shadow: 0 0 0 1px --warm`
- Sink node: `background: --surface`
- Wires: 1.4px stroke matching port color; stream wires are `stroke-dasharray: 4 3`; fun wires are 2px
- Rank badges (`@`): tiny `--warm` bordered superscript beside a port
- Adverb toggles: small bordered label in node header, active adverbs use `--warm` color/border

### Syntax Highlighting (identical in all views)

| Token | Color | Weight |
|---|---|---|
| operators | `--port-signal` | 400 |
| `\`, `=`, `=>` (lambda/bind) | `--port-fun` | 500 |
| strings | `--port-stream` | 400 |
| `.sym`, `,sym`, `'sym` | `--port-fun` | 400 |
| numbers | `--ink` | 400 |
| brackets `[ ]` | `--ink-2` | 400 |
| built-ins | `--ink` | 500 |
| user names | `--ink` | 400 |
| comments | `--ink-3` | 400 |

## Milestone Status

**M0 (Scaffold)** ✅ — workspace compiles, arithmetic ops (`+`, `-`, `*`, `/`), stack ops (`dup`, `drop`, `swap`, `over`), parser handles numbers and bare words, REPL and smoke test work.

**M1 (Interpreter core)** ✅ — 365/365 (100%) of McCartney's SAPF unit tests passing. Implemented: full arithmetic/logic (`&`, `|`), adverbs (`@`, `/`, `\`, `^`), streams (`ord`, `ordz`, `nat`, `by`, `cyc`, `skip`, `N`, `take`, `drop`, `nby`), list/tuple/sort/grade/flatten/mirror/shift/rot ops, `?` filter (including lazy infinite×infinite), `skipWhile`/`keepWhile`, Form ops (`has`, `keys`, `values`, `kv`, `parent`, `local`, `dot`), refs, signals.

**M2 (Rank-polymorphic dispatch)** ✅ — `@`/`@@`/`@@@` (each with depth), `@1`/`@2` outer products, zip mode (two `@` calls), lazy adverbs on infinite streams, auto-mapping `to`/`by`/`N` over Stream args, `GenSignal` for lazy infinite signals, `automap_unary` recurses into nested streams. Deterministic seeds: `seed` (set RNG seed), `rand` (seeded [0,1)), `irand` (seeded int), `muss` now uses session seed. All 365 SAPF unit tests pass.

**M3 (Audio runtime + MIDI/OSC + DSP alignment + extended DSP)** ✅ — `stax-dsp`: oscillators (`sinosc`, `saw`/`lfsaw`, `tri`, `square`, `pulse`, `impulse`), noise (`wnoise`/`white`, `pnoise`/`pink`, `brown`, `lfnoise0`, `lfnoise1`, `dust`, `dust2`, `sah`), filters (`lpf1`, `lpf`/`lpf2`, `hpf1`, `hpf`/`hpf2`, `rlpf`, `rhpf`, `lag`, `lag2`, `leakdc`), control signals (`line`, `xline`, `decay`), Karplus-Strong (`pluck`), comb filter (`combn`), envelopes (`ar`, `adsr`, `fadein`, `fadeout`, `hanenv`, `decay2`), delay (`delayn`), pan (`pan2`, `bal2`, `rot2`, `pan3`), sample-rate conversion (`upSmp`, `dwnSmp`), disperser (cascaded allpass), FFT/IFFT. `BinarySignal`/`UnarySignal`/`ConstSignal` enable lazy signal composition. Fixed-block audio via cpal (`play`/`stop`), MIDI out via midir, OSC via rosc. Sample-rate words: `sr`, `nyq`, `isr`, `inyq`, `rps`. Conversion: `midihz`, `midinote`, `bilin`, `biexp`, `linlin`, `linexp`, `explin`. Math extensions: `sign`, `hypot`, `clip`, `wrap`, `fold`, `dbtamp`, `amptodb`, `sinc`. Signal analysis: `normalize`, `peak`, `rms`, `dur`. Random: `rands`, `irands`, `picks`, `coins`. Debug: `p`, `trace`, `inspect`, `bench`. Stream generators: `fib`, `primes`, `logistic` (r x0 → chaotic [0,1] stream), `henon` (a b x0 y0 → [x,y] pair stream). Strange attractors (RK4 Signal generators): `lorenz` (→[x,y,z] signals), `rossler` (→[x,y,z] signals), `duffing` (→Signal x), `vanderpol` (→Signal x). List ops: `grow`, `ngrow`, `lindiv`, `expdiv`, `ever`, `lace`, `2X`. ZList words: `natz`, `byz`, `nbyz`, `invz`, `negz`, `evenz`, `oddz`. **Tier 1 DSP additions:** SVF filter (`svflp`/`svfhp`/`svfbp`/`svfnotch` — Chamberlin topology), compressor/limiter, window functions (`hann`, `hamming`, `blackman`, `blackmanharris`, `nuttall`, `flattop`, `gaussian`, `kaiser`), Hilbert FIR (`hilbert`), windowed-sinc FIR design (`firlp`/`firhp`/`firbp`), FDN reverb (`verb` — Jot/Hadamard), waveshaping (`tanhsat`, `softclip`, `hardclip`, `cubicsat`, `atansat`, `chebdist`). **Tier 2 DSP additions:** phase vocoder (`pvocstretch`, `pvocp`), granular synthesis (`grain`), LPC analysis/synthesis (`lpcanalz`/`lpcsynth`), Goertzel (`goertzel`/`goertzelc`), MDCT/IMDCT (`mdct`/`imdct`), Thiran allpass fractional delay (`thiran`), Farrow variable fractional delay (`farrow`), CQT (`cqt`). **Deferred:** synchrosqueezing (requires CWT; in PLAN.md as M3.5). 94 unit tests + 6 stax-dsp tests + 365/365 SAPF tests = 100 total, all pass.

**M4 (Graph IR + round-trip)** ✅ — `stax-graph`: `NodeId`, `EdgeId`, `PortRef`, `PortKind`, `Port`, `Node`/`NodeKind`, `Edge`, `Graph`. `lift(ops) → Graph` simulates the stack symbolically to produce data-flow edges. `lower(graph) → Vec<Op>` emits nodes in insertion order (= original parse order) for trivially correct round-trips. `lower_ordered(graph, iter)` accepts an explicit ordering for M5 editor use. `topo_sort(graph) → Vec<NodeId>` provides Kahn's algorithm with insertion-order tie-breaking for the M5 editor. Built-in arity table covers all M3 words. `Adverb` ops are folded into the consuming `Word` node and re-emitted by lower. Round-trip invariant: `lower(lift(parse(s)))` evaluates identically to `parse(s)`. 36 graph tests pass (identity, structure, topo-sort, semantic round-trip). Total test count: 136 (100 existing + 36 new).
