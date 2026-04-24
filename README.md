# stax

[![CI](https://github.com/9LiveZZZ-Git/STAX/actions/workflows/ci.yml/badge.svg)](https://github.com/9LiveZZZ-Git/STAX/actions/workflows/ci.yml)
[![Deploy](https://github.com/9LiveZZZ-Git/STAX/actions/workflows/pages.yml/badge.svg)](https://9LiveZZZ-Git.github.io/STAX/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)
[![SAPF Compatibility](https://img.shields.io/badge/SAPF-365%2F365-brightgreen)](https://github.com/lfnoise/sapf)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg)](https://www.rust-lang.org/)

**[Try the editor in your browser →](https://9LiveZZZ-Git.github.io/STAX/)**

A Rust reimplementation of **SAPF** (Sound As Pure Form) — James McCartney's stack-based concatenative audio programming language — with a working node-based visual editor, GPU operators, and JIT-compiled signal paths on the roadmap.

---

## What is SAPF?

[SAPF](https://github.com/lfnoise/sapf) is a language by [James McCartney](https://github.com/lfnoise) (creator of SuperCollider) in which sound is expressed as pure mathematical form. Programs are terse postfix expressions where signals, streams, functions, and data share a single unified stack:

```
// 440 Hz sine, amplitude-modulated by a 2 Hz LFO
440 sinosc  2 sinosc 0.5 * 0.5 +  *  play
```

SAPF treats audio-rate signals and lazy lists uniformly through APL-style rank-lifting operators (`@` map, `+/` reduce, `+\` scan) and first-class functions, giving it remarkable expressive density.

## What is stax?

stax is a faithful Rust port of the SAPF language (passes all 365 of McCartney's unit tests) with extensions that go beyond the original:

| Feature | SAPF | stax |
|---|---|---|
| Stack-based concatenative language | ✅ | ✅ |
| Streams, signals, functions, forms | ✅ | ✅ |
| APL-style rank operators | ✅ | ✅ |
| Cross-platform audio (cpal) | — | ✅ |
| MIDI out / OSC | — | ✅ |
| SVF, FDN reverb, phase vocoder, granular | — | ✅ |
| CQT, MDCT, Thiran, Farrow, LPC, Goertzel | — | ✅ |
| Strange attractors (Lorenz, Rössler, Duffing) | — | ✅ |
| Node-based visual editor (egui) | — | ✅ |
| GPU operators | — | M9 |
| JIT-compiled signal paths | — | M10 |

---

## Status

| Milestone | Description | Status |
|---|---|---|
| **M0** | Workspace scaffold, arithmetic REPL, audio pipeline | ✅ |
| **M1** | Full interpreter core — 365/365 SAPF unit tests | ✅ |
| **M2** | Rank-polymorphic dispatch, `@`/`@@`/`@@@`, outer products, deterministic seeds | ✅ |
| **M3** | Audio runtime (cpal), MIDI/OSC, full DSP suite, Tier 1/2 extended DSP | ✅ |
| **M4** | Graph IR + text↔graph round-trip (`stax-graph`) | ✅ |
| **M5** | egui editor: graph view, text view, embedded REPL | ✅ |
| **M6** | WASM browser target — editor runs in browser via GitHub Pages; AudioWorklet audio bridge | ⚡ |
| **M7** | Arrangement, clips, automation | 🔲 |
| **M8** | VST/AU plugin target | 🔲 |
| **M9** | GPU operators | 🔲 |
| **M10** | JIT-compiled signal paths | 🔲 |

**Current test count: 546 passing** (365 SAPF integration + 94 interpreter + 72 parser stress + 36 graph + 6 DSP + others).

---

## Quickstart

**Browser (no install):** open **[https://9LiveZZZ-Git.github.io/STAX/](https://9LiveZZZ-Git.github.io/STAX/)** — the full graph + text editor runs as a compiled WASM app. Audio output requires the native build for now (AudioWorklet bridge is M6 in-progress).

**Native:**

```sh
# Verify workspace builds
cargo check --workspace

# Play a 440 Hz sine for one second
cargo run --bin stax-smoke

# Interactive REPL
cargo run --bin stax-repl
> 2 3 +
5
> 440 sinosc play
# ... audio plays ...

# Visual editor (graph + text views)
cargo run --bin stax-editor
```

**Linux users:** ALSA headers are required for audio.

```sh
sudo apt-get install libasound2-dev libjack-jackd2-dev
```

---

## Editor (M5)

The `stax-editor` binary is a native desktop app (eframe/egui) implementing two views over the same program:

**Graph view** — the default. Each word in the program is a node; data-flow edges are wires coloured by type (terracotta = signal, teal = stream, violet = function, black = scalar). The canvas supports pan (Alt+drag or middle-mouse), zoom (scroll, cursor-centred), and click-to-select with node drag.

**Text view** — syntax-highlighted source with a files/outline sidebar and a stack inspector + REPL on the right. Switch views with the tab strip at the top.

**Embedded REPL** — available in both views. Type any stax expression and press Enter; the result appears immediately. Special commands: `.s` (show stack), `.c` (clear stack).

The editor design follows a "refined brutalism" aesthetic derived from `design/*.html` — no rounded corners, no shadows, port-type colours appear nowhere else in the UI, and `--warm` (terracotta) marks only active/selected state.

---

## Language Sample

```
// Arithmetic and stack operations
2 3 +              // → 5
10 2 - 3 *         // → 24

// Lazy streams
ord 5 N            // → [1 2 3 4 5]
nat 3 to cyc 8 N   // → [1 2 3 1 2 3 1 2]

// Rank-lifting
[1 2 3] [10 20 30] + @    // → [11 22 33]  (element-wise)
[1 2 3] [10 20 30] *@1@2  // → [[10 20 30] [20 40 60] [30 60 90]]  (outer product)

// Audio
440 sinosc                                    // sine oscillator
440 sinosc 2.0 sinosc 0.5 * 0.5 + *  play    // AM synthesis
0.01 0.1 ar  440 sinosc * play                // attack-release envelope

// SVF filter, reverb, compression
440 sinosc  2000 0.7 svflp                    // state-variable LP filter
440 sinosc  8 2.0 0.5 verb                    // FDN reverb
440 sinosc  -20 4 0.01 0.1 0 compressor       // compressor

// Strange attractors (audio-rate chaos)
10 28 2.667 0.005 0.1 0 0 lorenz              // → [x_sig y_sig z_sig]
3.99 0.5 logistic 44 N                        // logistic map stream

// Analysis
440 sinosc 4096 N Z  440 goertzel             // Goertzel frequency detection
440 sinosc 1024 N Z  12 220 24 cqt            // Constant-Q Transform
```

---

## Architecture

```
stax-parser ──► Vec<Op> ◄── stax-graph  ◄── stax-editor
                   │             │                │
              stax-eval     lift/lower        graph + text
                   │                           views, REPL
         ┌─────────┼─────────┐
      stax-dsp  stax-audio  stax-io
    (DSP prims)  (cpal)   (MIDI/OSC)
```

The single `Vec<Op>` IR is what makes lossless text↔graph round-tripping possible. Both the text parser and the graph editor emit `Vec<Op>`; the interpreter consumes it.

### Crate map

| Crate | Role | Status |
|---|---|---|
| `stax-core` | `Value`, `Stream`/`Signal` traits, `Op` IR | ✅ |
| `stax-parser` | postfix text → `Vec<Op>` | ✅ |
| `stax-eval` | stack machine + all built-in words | ✅ |
| `stax-dsp` | oscillators, filters, attractors, full DSP suite | ✅ |
| `stax-audio` | cpal native runtime + WASM stub | ✅ |
| `stax-io` | MIDI out (midir) + OSC (rosc) | ✅ |
| `stax-graph` | graph IR + `lift`/`lower` round-trip | ✅ |
| `stax-editor` | egui node editor — graph + text views | ✅ |
| `stax-arrange` | transport, clips, automation | M7 |
| `stax-plugin` | VST/AU target | M8 |
| `stax-gpu` | GPU operators | M9 |
| `stax-jit` | JIT signal paths | M10 |

---

## M3 DSP Reference

<details>
<summary>Oscillators</summary>

`sinosc` `saw`/`lfsaw` `tri` `square` `pulse` `impulse`
</details>

<details>
<summary>Noise & stochastic</summary>

`wnoise`/`white` `pnoise`/`pink` `brown` `lfnoise0` `lfnoise1` `dust` `dust2` `sah`
</details>

<details>
<summary>Filters</summary>

`lpf1` `lpf`/`lpf2` `hpf1` `hpf`/`hpf2` `rlpf` `rhpf` `lag` `lag2` `leakdc`
`svflp` `svfhp` `svfbp` `svfnotch` — Chamberlin state-variable filter
`firlp` `firhp` `firbp` — windowed-sinc FIR design
`hilbert` — Hilbert FIR (63-tap quadrature)
`disperser` — cascaded allpass phase disperser
`thiran` — Thiran allpass fractional delay
`farrow` — Farrow variable fractional delay
</details>

<details>
<summary>Envelopes & control</summary>

`ar` `adsr` `fadein` `fadeout` `hanenv` `decay` `decay2` `line` `xline`
</details>

<details>
<summary>Reverb, delay, dynamics</summary>

`combn` — comb filter
`delayn` — fixed sample delay
`verb` — FDN reverb (Jot/Hadamard, N feedback lines)
`compressor` `limiter`
</details>

<details>
<summary>Waveshaping</summary>

`tanhsat` `softclip` `hardclip` `cubicsat` `atansat` `chebdist`
</details>

<details>
<summary>Spatial</summary>

`pan2` `bal2` `rot2` `pan3`
</details>

<details>
<summary>Synthesis</summary>

`pluck` — Karplus-Strong
`grain` — granular synthesis
`pvocstretch` `pvocp` — phase vocoder (time-stretch / pitch-shift)
</details>

<details>
<summary>Windows</summary>

`hann` `hamming` `blackman` `blackmanharris` `nuttall` `flattop` `gaussian` `kaiser`
</details>

<details>
<summary>Analysis & transforms</summary>

`goertzel` `goertzelc` — single-frequency DFT
`cqt` — Constant-Q Transform (preferred over FFT for pitch analysis)
`mdct` `imdct` — Modified Discrete Cosine Transform
`lpcanalz` `lpcsynth` — Linear Predictive Coding
`fft` `ifft` — realfft-backed magnitude spectrum
`normalize` `peak` `rms` `dur`
</details>

<details>
<summary>Strange attractors (RK4)</summary>

`lorenz` `rossler` `duffing` `vanderpol`
`logistic` `henon` — discrete chaotic maps
</details>

---

## Contributing

Contributions are welcome. The project follows standard Rust conventions:

- `cargo fmt` before committing
- `cargo clippy --workspace -- -D warnings` must pass
- All new features need tests; all existing tests must pass

See [PLAN.md](PLAN.md) for the roadmap and [CLAUDE.md](CLAUDE.md) for Claude Code guidance.

---

## Attribution

stax is built on top of **SAPF** (Sound As Pure Form) by **James McCartney**.

- SAPF repository: <https://github.com/lfnoise/sapf>
- The 365 unit tests in `crates/stax-eval/tests/sapf_unit.rs` are derived from McCartney's `unit-tests.txt`.
- The language semantics, stack model, and APL-style rank operators originate entirely with McCartney's design.

stax adds a Rust implementation, extended DSP, cross-platform audio I/O, and a visual editor — but the language itself is his.

---

## License

Licensed under the [MIT License](LICENSE-MIT).
