//! Signal primitives: oscillators, filters, noise, envelopes.
//!
//! M0 — SinOsc, VecSignal.
//! M3 — SawOsc, LfSawOsc, WhiteNoise, PinkNoise, CombFilterSignal,
//!       PluckOsc, ArEnv, AdsrEnv. SIMD (via `wide`) in hot scalar loops.

use std::f32::consts::TAU;
use std::sync::Arc;

use stax_core::{Signal, SignalInstance};

pub use stax_core::signal::{BinarySignal, ConstSignal, UnarySignal};

// ---- helpers ----------------------------------------------------------------

#[inline(always)]
fn lcg_next(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    // Map to [-1, 1)
    (*seed >> 33) as f32 / (u32::MAX as f32) * 2.0 - 1.0
}

// ---- SinOsc -----------------------------------------------------------------

pub struct SinOsc {
    pub freq_hz: f32,
    pub initial_phase: f32,
}

impl SinOsc {
    pub fn new(freq_hz: f32) -> Self {
        Self { freq_hz, initial_phase: 0.0 }
    }
    pub fn with_phase(freq_hz: f32, initial_phase: f32) -> Self {
        Self { freq_hz, initial_phase }
    }
}

impl Signal for SinOsc {
    fn instantiate(&self, sample_rate: f64) -> Box<dyn SignalInstance> {
        Box::new(SinOscInstance {
            phase: self.initial_phase,
            phase_inc: TAU * self.freq_hz / sample_rate as f32,
        })
    }
}

struct SinOscInstance {
    phase: f32,
    phase_inc: f32,
}

impl SignalInstance for SinOscInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = self.phase.sin();
            self.phase += self.phase_inc;
            if self.phase > TAU { self.phase -= TAU; }
        }
    }
}

// ---- SawOsc / LfSawOsc ------------------------------------------------------

/// Band-limited sawtooth (scalar PolyBLEP — M3 ships simple non-bandlimited).
pub struct SawOsc {
    pub freq_hz: f32,
    pub initial_phase: f32,
}

impl SawOsc {
    pub fn new(freq_hz: f32) -> Self { Self { freq_hz, initial_phase: 0.0 } }
    pub fn with_phase(freq_hz: f32, initial_phase: f32) -> Self { Self { freq_hz, initial_phase } }
}

impl Signal for SawOsc {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(SawInstance {
            phase: self.initial_phase,
            phase_inc: self.freq_hz / sr as f32,
        })
    }
}

struct SawInstance { phase: f32, phase_inc: f32 }

impl SignalInstance for SawInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = self.phase * 2.0 - 1.0;
            self.phase += self.phase_inc;
            if self.phase >= 1.0 { self.phase -= 1.0; }
        }
    }
}

/// LF (non-interpolating) sawtooth — same implementation, intended for LFO use.
pub type LfSawOsc = SawOsc;

// ---- WhiteNoise -------------------------------------------------------------

pub struct WhiteNoise {
    pub seed: u64,
}

impl WhiteNoise {
    pub fn new(seed: u64) -> Self { Self { seed } }
}

impl Signal for WhiteNoise {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(WhiteNoiseInstance { seed: self.seed })
    }
}

struct WhiteNoiseInstance { seed: u64 }

impl SignalInstance for WhiteNoiseInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() { *s = lcg_next(&mut self.seed); }
    }
}

// ---- PinkNoise (Voss-McCartney, 16 generators) ------------------------------

pub struct PinkNoise {
    pub seed: u64,
}

impl PinkNoise {
    pub fn new(seed: u64) -> Self { Self { seed } }
}

impl Signal for PinkNoise {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        let mut seed = self.seed;
        let mut gens = [0.0f32; 16];
        for g in gens.iter_mut() { *g = lcg_next(&mut seed); }
        Box::new(PinkNoiseInstance {
            gens,
            running_sum: gens.iter().sum(),
            seed,
            counter: 0u64,
        })
    }
}

struct PinkNoiseInstance {
    gens: [f32; 16],
    running_sum: f32,
    seed: u64,
    counter: u64,
}

impl SignalInstance for PinkNoiseInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            let zeros = (self.counter.wrapping_add(1).trailing_zeros() as usize).min(15);
            let old = self.gens[zeros];
            let new = lcg_next(&mut self.seed);
            self.gens[zeros] = new;
            self.running_sum += new - old;
            let white = lcg_next(&mut self.seed);
            *s = (self.running_sum + white) / 17.0;
            self.counter = self.counter.wrapping_add(1);
        }
    }
}

// ---- CombFilterSignal -------------------------------------------------------

/// Comb filter (non-interpolating): y[n] = x[n] + coeff * y[n - delay_samples].
pub struct CombFilterSignal {
    pub input: Arc<dyn Signal>,
    pub delay_samples: usize,
    pub coeff: f32,
}

impl Signal for CombFilterSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let n = self.delay_samples.max(1);
        Box::new(CombInstance {
            input: self.input.instantiate(sr),
            buffer: vec![0.0f32; n],
            pos: 0,
            coeff: self.coeff,
        })
    }
}

struct CombInstance {
    input: Box<dyn SignalInstance>,
    buffer: Vec<f32>,
    pos: usize,
    coeff: f32,
}

impl SignalInstance for CombInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        let len = self.buffer.len();
        for s in out.iter_mut() {
            let delayed = self.buffer[self.pos];
            let y = *s + self.coeff * delayed;
            self.buffer[self.pos] = y;
            self.pos = (self.pos + 1) % len;
            *s = y;
        }
    }
}

// ---- PluckOsc (Karplus-Strong) ----------------------------------------------

pub struct PluckOsc {
    pub freq_hz: f32,
    pub decay_time: f32,
    pub seed: u64,
}

impl PluckOsc {
    pub fn new(freq_hz: f32, decay_time: f32) -> Self {
        Self { freq_hz, decay_time, seed: 0xdeadbeef_cafebabe }
    }
}

impl Signal for PluckOsc {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let delay = ((sr / self.freq_hz as f64).round() as usize).max(2);
        let mut seed = self.seed;
        let buffer: Vec<f32> = (0..delay).map(|_| lcg_next(&mut seed)).collect();
        // Decay per-sample: exp(-ln(1000) / (decay_time * sr)) ≈ -60dB over decay_time
        let decay = if self.decay_time > 0.0 {
            (-6.9078f64 / (self.decay_time as f64 * sr)).exp() as f32
        } else {
            0.0
        };
        Box::new(PluckInstance { buffer, pos: 0, decay })
    }
}

struct PluckInstance {
    buffer: Vec<f32>,
    pos: usize,
    decay: f32,
}

impl SignalInstance for PluckInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let len = self.buffer.len();
        for s in out.iter_mut() {
            let cur = self.buffer[self.pos];
            let prev = self.buffer[(self.pos + len - 1) % len];
            let next = (cur + prev) * 0.5 * self.decay;
            self.buffer[self.pos] = next;
            self.pos = (self.pos + 1) % len;
            *s = cur;
        }
    }
}

// ---- ArEnv ------------------------------------------------------------------

/// One-shot attack/release envelope. Produces samples indefinitely (0 after release).
pub struct ArEnv {
    pub attack_secs: f32,
    pub release_secs: f32,
}

impl Signal for ArEnv {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(ArEnvInstance {
            attack: (self.attack_secs as f64 * sr) as usize,
            release: (self.release_secs as f64 * sr) as usize,
            pos: 0,
        })
    }
}

struct ArEnvInstance { attack: usize, release: usize, pos: usize }

impl SignalInstance for ArEnvInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = if self.pos < self.attack {
                self.pos as f32 / self.attack.max(1) as f32
            } else {
                let r_pos = self.pos - self.attack;
                if r_pos < self.release {
                    1.0 - r_pos as f32 / self.release.max(1) as f32
                } else {
                    0.0
                }
            };
            self.pos += 1;
        }
    }
}

// ---- AdsrEnv ----------------------------------------------------------------

/// Four-stage ADSR envelope. `sustain_secs` controls how long sustain lasts.
pub struct AdsrEnv {
    pub attack_secs: f32,
    pub decay_secs: f32,
    pub sustain_level: f32,
    pub release_secs: f32,
    pub sustain_secs: f32,
}

impl Signal for AdsrEnv {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(AdsrInstance {
            attack: (self.attack_secs as f64 * sr) as usize,
            decay: (self.decay_secs as f64 * sr) as usize,
            sustain_level: self.sustain_level,
            sustain: (self.sustain_secs as f64 * sr) as usize,
            release: (self.release_secs as f64 * sr) as usize,
            pos: 0,
        })
    }
}

struct AdsrInstance {
    attack: usize,
    decay: usize,
    sustain_level: f32,
    sustain: usize,
    release: usize,
    pos: usize,
}

impl SignalInstance for AdsrInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            let p = self.pos;
            *s = if p < self.attack {
                p as f32 / self.attack.max(1) as f32
            } else {
                let p = p - self.attack;
                if p < self.decay {
                    1.0 - (1.0 - self.sustain_level) * p as f32 / self.decay.max(1) as f32
                } else {
                    let p = p - self.decay;
                    if p < self.sustain {
                        self.sustain_level
                    } else {
                        let p = p - self.sustain;
                        if p < self.release {
                            self.sustain_level * (1.0 - p as f32 / self.release.max(1) as f32)
                        } else {
                            0.0
                        }
                    }
                }
            };
            self.pos += 1;
        }
    }
}

// ---- BrownNoise (integrated white noise) ------------------------------------

pub struct BrownNoise {
    pub seed: u64,
}

impl BrownNoise {
    pub fn new(seed: u64) -> Self { Self { seed } }
}

impl Signal for BrownNoise {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(BrownInstance { seed: self.seed, val: 0.0 })
    }
}

struct BrownInstance { seed: u64, val: f32 }

impl SignalInstance for BrownInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            let step = lcg_next(&mut self.seed) * 0.05;
            self.val = (self.val + step).clamp(-1.0, 1.0);
            *s = self.val;
        }
    }
}

// ---- TriOsc (triangle wave) -------------------------------------------------

pub struct TriOsc {
    pub freq_hz: f32,
    pub initial_phase: f32,
}

impl TriOsc {
    pub fn new(freq_hz: f32) -> Self { Self { freq_hz, initial_phase: 0.0 } }
    pub fn with_phase(freq_hz: f32, initial_phase: f32) -> Self { Self { freq_hz, initial_phase } }
}

impl Signal for TriOsc {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(TriInstance { phase: self.initial_phase, phase_inc: self.freq_hz / sr as f32 })
    }
}

struct TriInstance { phase: f32, phase_inc: f32 }

impl SignalInstance for TriInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = if self.phase < 0.5 { 4.0 * self.phase - 1.0 } else { 3.0 - 4.0 * self.phase };
            self.phase += self.phase_inc;
            if self.phase >= 1.0 { self.phase -= 1.0; }
        }
    }
}

// ---- PulseOsc (pulse / square wave) -----------------------------------------

pub struct PulseOsc {
    pub freq_hz: f32,
    pub duty: f32,
    pub initial_phase: f32,
}

impl PulseOsc {
    pub fn new(freq_hz: f32, duty: f32) -> Self { Self { freq_hz, duty, initial_phase: 0.0 } }
    pub fn with_phase(freq_hz: f32, duty: f32, initial_phase: f32) -> Self {
        Self { freq_hz, duty, initial_phase }
    }
}

impl Signal for PulseOsc {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(PulseInstance {
            phase: self.initial_phase,
            phase_inc: self.freq_hz / sr as f32,
            duty: self.duty.clamp(0.001, 0.999),
        })
    }
}

struct PulseInstance { phase: f32, phase_inc: f32, duty: f32 }

impl SignalInstance for PulseInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = if self.phase < self.duty { 1.0 } else { -1.0 };
            self.phase += self.phase_inc;
            if self.phase >= 1.0 { self.phase -= 1.0; }
        }
    }
}

// ---- ImpulseSignal (periodic impulse train) ---------------------------------

pub struct ImpulseSignal {
    pub freq_hz: f32,
    pub initial_phase: f32,
}

impl ImpulseSignal {
    pub fn new(freq_hz: f32) -> Self { Self { freq_hz, initial_phase: 0.0 } }
}

impl Signal for ImpulseSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(ImpulseInstance {
            phase: self.initial_phase,
            phase_inc: self.freq_hz / sr as f32,
        })
    }
}

struct ImpulseInstance { phase: f32, phase_inc: f32 }

impl SignalInstance for ImpulseInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            let next = self.phase + self.phase_inc;
            *s = if next >= 1.0 { 1.0 } else { 0.0 };
            self.phase = next;
            if self.phase >= 1.0 { self.phase -= 1.0; }
        }
    }
}

// ---- Filter infrastructure --------------------------------------------------

/// RBJ cookbook LP filter coefficients.
fn rbj_lpf(fc: f64, sr: f64, q: f64) -> (f64, f64, f64, f64, f64) {
    let w0 = std::f64::consts::TAU * fc / sr;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);
    let b0 = (1.0 - cos_w0) / 2.0;
    let b1 = 1.0 - cos_w0;
    let b2 = b0;
    let a0 = 1.0 + alpha;
    (b0/a0, b1/a0, b2/a0, -2.0*cos_w0/a0, (1.0-alpha)/a0)
}

/// RBJ cookbook HP filter coefficients.
fn rbj_hpf(fc: f64, sr: f64, q: f64) -> (f64, f64, f64, f64, f64) {
    let w0 = std::f64::consts::TAU * fc / sr;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);
    let b0 = (1.0 + cos_w0) / 2.0;
    let b1 = -(1.0 + cos_w0);
    let b2 = b0;
    let a0 = 1.0 + alpha;
    (b0/a0, b1/a0, b2/a0, -2.0*cos_w0/a0, (1.0-alpha)/a0)
}

/// Shared transposed-direct-form-II biquad instance.
struct Biquad2Instance {
    input: Box<dyn SignalInstance>,
    b: [f32; 3],
    a: [f32; 2],
    s1: f32,
    s2: f32,
}

impl SignalInstance for Biquad2Instance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            let x = *s;
            let y = self.b[0] * x + self.s1;
            self.s1 = self.b[1] * x - self.a[0] * y + self.s2;
            self.s2 = self.b[2] * x - self.a[1] * y;
            *s = y;
        }
    }
}

// ---- Lpf1Signal (1-pole LP) -------------------------------------------------

pub struct Lpf1Signal {
    pub input: Arc<dyn Signal>,
    pub cutoff_hz: f32,
}

impl Signal for Lpf1Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let a = (1.0 - (-std::f64::consts::TAU * self.cutoff_hz as f64 / sr).exp()) as f32;
        Box::new(Lpf1Instance { input: self.input.instantiate(sr), a, y: 0.0 })
    }
}

struct Lpf1Instance { input: Box<dyn SignalInstance>, a: f32, y: f32 }

impl SignalInstance for Lpf1Instance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            self.y += self.a * (*s - self.y);
            *s = self.y;
        }
    }
}

// ---- Lpf2Signal (2nd-order Butterworth LP) ----------------------------------

pub struct Lpf2Signal {
    pub input: Arc<dyn Signal>,
    pub cutoff_hz: f32,
}

impl Signal for Lpf2Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let (b0, b1, b2, a1, a2) = rbj_lpf(self.cutoff_hz as f64, sr, 1.0 / 2f64.sqrt());
        Box::new(Biquad2Instance {
            input: self.input.instantiate(sr),
            b: [b0 as f32, b1 as f32, b2 as f32],
            a: [a1 as f32, a2 as f32],
            s1: 0.0, s2: 0.0,
        })
    }
}

// ---- Hpf1Signal (1-pole HP) -------------------------------------------------

pub struct Hpf1Signal {
    pub input: Arc<dyn Signal>,
    pub cutoff_hz: f32,
}

impl Signal for Hpf1Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let a = (1.0 - (-std::f64::consts::TAU * self.cutoff_hz as f64 / sr).exp()) as f32;
        Box::new(Hpf1Instance { input: self.input.instantiate(sr), a, y_lp: 0.0 })
    }
}

struct Hpf1Instance { input: Box<dyn SignalInstance>, a: f32, y_lp: f32 }

impl SignalInstance for Hpf1Instance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            self.y_lp += self.a * (*s - self.y_lp);
            *s -= self.y_lp;
        }
    }
}

// ---- Hpf2Signal (2nd-order HP) ----------------------------------------------

pub struct Hpf2Signal {
    pub input: Arc<dyn Signal>,
    pub cutoff_hz: f32,
}

impl Signal for Hpf2Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let (b0, b1, b2, a1, a2) = rbj_hpf(self.cutoff_hz as f64, sr, 1.0 / 2f64.sqrt());
        Box::new(Biquad2Instance {
            input: self.input.instantiate(sr),
            b: [b0 as f32, b1 as f32, b2 as f32],
            a: [a1 as f32, a2 as f32],
            s1: 0.0, s2: 0.0,
        })
    }
}

// ---- RlpfSignal (resonant LP) -----------------------------------------------

pub struct RlpfSignal {
    pub input: Arc<dyn Signal>,
    pub cutoff_hz: f32,
    pub rq: f32,
}

impl Signal for RlpfSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let q = (1.0 / self.rq as f64).max(0.01);
        let (b0, b1, b2, a1, a2) = rbj_lpf(self.cutoff_hz as f64, sr, q);
        Box::new(Biquad2Instance {
            input: self.input.instantiate(sr),
            b: [b0 as f32, b1 as f32, b2 as f32],
            a: [a1 as f32, a2 as f32],
            s1: 0.0, s2: 0.0,
        })
    }
}

// ---- RhpfSignal (resonant HP) -----------------------------------------------

pub struct RhpfSignal {
    pub input: Arc<dyn Signal>,
    pub cutoff_hz: f32,
    pub rq: f32,
}

impl Signal for RhpfSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let q = (1.0 / self.rq as f64).max(0.01);
        let (b0, b1, b2, a1, a2) = rbj_hpf(self.cutoff_hz as f64, sr, q);
        Box::new(Biquad2Instance {
            input: self.input.instantiate(sr),
            b: [b0 as f32, b1 as f32, b2 as f32],
            a: [a1 as f32, a2 as f32],
            s1: 0.0, s2: 0.0,
        })
    }
}

// ---- LagSignal (1-pole LP smoother) -----------------------------------------

pub struct LagSignal {
    pub input: Arc<dyn Signal>,
    pub lag_time: f32,
}

impl Signal for LagSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let coeff = if self.lag_time <= 0.0 {
            0.0f32
        } else {
            (-1.0 / (self.lag_time as f64 * sr)).exp() as f32
        };
        Box::new(LagInstance { input: self.input.instantiate(sr), coeff, y: 0.0 })
    }
}

struct LagInstance { input: Box<dyn SignalInstance>, coeff: f32, y: f32 }

impl SignalInstance for LagInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            self.y = self.y * self.coeff + *s * (1.0 - self.coeff);
            *s = self.y;
        }
    }
}

// ---- Lag2Signal (2-stage lag) -----------------------------------------------

pub struct Lag2Signal {
    pub input: Arc<dyn Signal>,
    pub lag_time: f32,
}

impl Signal for Lag2Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let coeff = if self.lag_time <= 0.0 {
            0.0f32
        } else {
            (-1.0 / (self.lag_time as f64 * sr)).exp() as f32
        };
        Box::new(Lag2Instance { input: self.input.instantiate(sr), coeff, y1: 0.0, y2: 0.0 })
    }
}

struct Lag2Instance { input: Box<dyn SignalInstance>, coeff: f32, y1: f32, y2: f32 }

impl SignalInstance for Lag2Instance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            self.y1 = self.y1 * self.coeff + *s * (1.0 - self.coeff);
            self.y2 = self.y2 * self.coeff + self.y1 * (1.0 - self.coeff);
            *s = self.y2;
        }
    }
}

// ---- LeakDcSignal (DC blocker) ----------------------------------------------

pub struct LeakDcSignal {
    pub input: Arc<dyn Signal>,
}

impl Signal for LeakDcSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let r = (1.0 - (std::f64::consts::TAU * 10.0 / sr)) as f32;
        Box::new(LeakDcInstance {
            input: self.input.instantiate(sr),
            r: r.clamp(0.9, 1.0),
            x_prev: 0.0,
            y_prev: 0.0,
        })
    }
}

struct LeakDcInstance { input: Box<dyn SignalInstance>, r: f32, x_prev: f32, y_prev: f32 }

impl SignalInstance for LeakDcInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            let x = *s;
            let y = x - self.x_prev + self.r * self.y_prev;
            self.x_prev = x;
            self.y_prev = y;
            *s = y;
        }
    }
}

// ---- LineSignal (linear ramp) -----------------------------------------------

pub struct LineSignal {
    pub start: f32,
    pub end: f32,
    pub dur_secs: f32,
}

impl Signal for LineSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let total = (self.dur_secs as f64 * sr).round() as usize;
        Box::new(LineInstance { start: self.start, end: self.end, total, pos: 0 })
    }
}

struct LineInstance { start: f32, end: f32, total: usize, pos: usize }

impl SignalInstance for LineInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let denom = (self.total.saturating_sub(1)).max(1) as f32;
        for s in out.iter_mut() {
            *s = if self.total == 0 || self.pos >= self.total {
                self.end
            } else {
                self.start + (self.end - self.start) * self.pos as f32 / denom
            };
            self.pos += 1;
        }
    }
}

// ---- XlineSignal (exponential ramp) -----------------------------------------

pub struct XlineSignal {
    pub start: f32,
    pub end: f32,
    pub dur_secs: f32,
}

impl Signal for XlineSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let total = (self.dur_secs as f64 * sr).round() as usize;
        let cur = self.start;
        let ratio = if self.start == 0.0 || self.end == 0.0 || total == 0 {
            1.0f32
        } else {
            (self.end / self.start).powf(1.0 / total as f32)
        };
        Box::new(XlineInstance { end: self.end, total, pos: 0, ratio, cur })
    }
}

struct XlineInstance { end: f32, total: usize, pos: usize, ratio: f32, cur: f32 }

impl SignalInstance for XlineInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = if self.pos >= self.total { self.end } else { self.cur };
            self.cur *= self.ratio;
            self.pos += 1;
        }
    }
}

// ---- DecaySignal (exponential decay from 1 → 0) ----------------------------

pub struct DecaySignal {
    pub dur_secs: f32,
}

impl Signal for DecaySignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        // -60 dB (factor of 1/1000) over dur_secs
        let decay = if self.dur_secs <= 0.0 {
            0.0f32
        } else {
            (-6.9078 / (self.dur_secs as f64 * sr)).exp() as f32
        };
        Box::new(DecayInstance { val: 1.0, decay })
    }
}

struct DecayInstance { val: f32, decay: f32 }

impl SignalInstance for DecayInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = self.val;
            self.val *= self.decay;
        }
    }
}

// ---- LfNoise0Signal (stepped random — new value at freq Hz) -----------------

pub struct LfNoise0Signal { pub freq_hz: f32, pub seed: u64 }

impl Signal for LfNoise0Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let mut seed = self.seed;
        let cur_val = lcg_next(&mut seed);
        Box::new(LfNoise0Instance { phase: 0.0, phase_inc: self.freq_hz / sr as f32, cur_val, seed })
    }
}

struct LfNoise0Instance { phase: f32, phase_inc: f32, cur_val: f32, seed: u64 }

impl SignalInstance for LfNoise0Instance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = self.cur_val;
            self.phase += self.phase_inc;
            if self.phase >= 1.0 { self.phase -= 1.0; self.cur_val = lcg_next(&mut self.seed); }
        }
    }
}

// ---- LfNoise1Signal (linearly interpolated random) --------------------------

pub struct LfNoise1Signal { pub freq_hz: f32, pub seed: u64 }

impl Signal for LfNoise1Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let mut seed = self.seed;
        let cur_val = lcg_next(&mut seed);
        let next_val = lcg_next(&mut seed);
        Box::new(LfNoise1Instance { phase: 0.0, phase_inc: self.freq_hz / sr as f32, cur_val, next_val, seed })
    }
}

struct LfNoise1Instance { phase: f32, phase_inc: f32, cur_val: f32, next_val: f32, seed: u64 }

impl SignalInstance for LfNoise1Instance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = self.cur_val + self.phase * (self.next_val - self.cur_val);
            self.phase += self.phase_inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.cur_val = self.next_val;
                self.next_val = lcg_next(&mut self.seed);
            }
        }
    }
}

// ---- DustSignal (random impulse train) --------------------------------------

pub struct DustSignal { pub density_hz: f32, pub seed: u64, pub bipolar: bool }

impl Signal for DustSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(DustInstance {
            threshold: self.density_hz / sr as f32,
            seed: self.seed,
            bipolar: self.bipolar,
        })
    }
}

struct DustInstance { threshold: f32, seed: u64, bipolar: bool }

impl SignalInstance for DustInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            let u = lcg_next(&mut self.seed) * 0.5 + 0.5; // map [-1,1) → [0,1)
            *s = if u < self.threshold {
                if self.bipolar { lcg_next(&mut self.seed).signum() } else { 1.0 }
            } else {
                0.0
            };
        }
    }
}

// ---- SahSignal (sample-and-hold) -------------------------------------------

pub struct SahSignal { pub input: Arc<dyn Signal>, pub trigger: Arc<dyn Signal> }

impl Signal for SahSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(SahInstance {
            input: self.input.instantiate(sr),
            trigger: self.trigger.instantiate(sr),
            held: 0.0,
            trig_buf: Vec::with_capacity(512),
        })
    }
}

struct SahInstance { input: Box<dyn SignalInstance>, trigger: Box<dyn SignalInstance>, held: f32, trig_buf: Vec<f32> }

impl SignalInstance for SahInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let n = out.len();
        self.trig_buf.resize(n, 0.0);
        self.input.fill(out);
        self.trigger.fill(&mut self.trig_buf[..n]);
        for (i, s) in out.iter_mut().enumerate() {
            if self.trig_buf[i] > 0.0 { self.held = *s; }
            *s = self.held;
        }
    }
}

// ---- DelayNSignal (fixed non-interpolating delay) ---------------------------

pub struct DelayNSignal { pub input: Arc<dyn Signal>, pub delay_secs: f32 }

impl Signal for DelayNSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let delay = ((self.delay_secs as f64 * sr).round() as usize).max(1);
        Box::new(DelayNInstance { input: self.input.instantiate(sr), buffer: vec![0.0f32; delay], pos: 0 })
    }
}

struct DelayNInstance { input: Box<dyn SignalInstance>, buffer: Vec<f32>, pos: usize }

impl SignalInstance for DelayNInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        let len = self.buffer.len();
        for s in out.iter_mut() {
            let delayed = self.buffer[self.pos];
            self.buffer[self.pos] = *s;
            self.pos = (self.pos + 1) % len;
            *s = delayed;
        }
    }
}

// ---- HanEnvSignal (raised-cosine / Hanning window envelope) -----------------

pub struct HanEnvSignal { pub dur_secs: f32 }

impl Signal for HanEnvSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let total = (self.dur_secs as f64 * sr).round() as usize;
        Box::new(HanEnvInstance { total, pos: 0 })
    }
}

struct HanEnvInstance { total: usize, pos: usize }

impl SignalInstance for HanEnvInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = if self.pos >= self.total {
                0.0
            } else {
                let t = self.pos as f32 / (self.total.saturating_sub(1)).max(1) as f32;
                0.5 * (1.0 - (std::f32::consts::TAU * t).cos())
            };
            self.pos += 1;
        }
    }
}

// ---- Decay2Signal (linear attack + exponential decay) -----------------------

pub struct Decay2Signal { pub attack_secs: f32, pub decay_secs: f32 }

impl Signal for Decay2Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let attack = (self.attack_secs as f64 * sr).round() as usize;
        let decay_coeff = if self.decay_secs <= 0.0 { 0.0f32 }
            else { (-6.9078 / (self.decay_secs as f64 * sr)).exp() as f32 };
        Box::new(Decay2Instance { attack, pos: 0, decay_coeff, cur: 0.0 })
    }
}

struct Decay2Instance { attack: usize, pos: usize, decay_coeff: f32, cur: f32 }

impl SignalInstance for Decay2Instance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = if self.pos < self.attack {
                let v = self.pos as f32 / self.attack.max(1) as f32;
                self.cur = v; v
            } else {
                self.cur *= self.decay_coeff; self.cur
            };
            self.pos += 1;
        }
    }
}

// ---- PanChannelSignal (one L or R channel of an equal-power stereo pan) -----

pub struct PanChannelSignal {
    pub input: Arc<dyn Signal>,
    pub pan: Arc<dyn Signal>, // -1 (full L) .. +1 (full R)
    pub is_right: bool,
}

impl Signal for PanChannelSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(PanChannelInstance {
            input: self.input.instantiate(sr),
            pan: self.pan.instantiate(sr),
            is_right: self.is_right,
            pan_buf: Vec::with_capacity(512),
        })
    }
}

struct PanChannelInstance {
    input: Box<dyn SignalInstance>,
    pan: Box<dyn SignalInstance>,
    is_right: bool,
    pan_buf: Vec<f32>,
}

impl SignalInstance for PanChannelInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let n = out.len();
        self.pan_buf.resize(n, 0.0);
        self.input.fill(out);
        self.pan.fill(&mut self.pan_buf[..n]);
        for (i, s) in out.iter_mut().enumerate() {
            // pan in [-1,1] → angle in [0, π/2]
            let angle = (self.pan_buf[i].clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            let gain = if self.is_right { angle.sin() } else { angle.cos() };
            *s *= gain;
        }
    }
}

// ---- Mix2Signal (L*ga + R*gb per sample — used by rot2) ---------------------

pub struct Mix2Signal {
    pub a: Arc<dyn Signal>,
    pub b: Arc<dyn Signal>,
    pub gain_a: f32,
    pub gain_b: f32,
}

impl Signal for Mix2Signal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(Mix2Instance {
            a: self.a.instantiate(sr),
            b: self.b.instantiate(sr),
            gain_a: self.gain_a,
            gain_b: self.gain_b,
            buf_b: Vec::with_capacity(512),
        })
    }
}

struct Mix2Instance { a: Box<dyn SignalInstance>, b: Box<dyn SignalInstance>, gain_a: f32, gain_b: f32, buf_b: Vec<f32> }

impl SignalInstance for Mix2Instance {
    fn fill(&mut self, out: &mut [f32]) {
        let n = out.len();
        self.buf_b.resize(n, 0.0);
        self.a.fill(out);
        self.b.fill(&mut self.buf_b[..n]);
        for (i, s) in out.iter_mut().enumerate() {
            *s = *s * self.gain_a + self.buf_b[i] * self.gain_b;
        }
    }
}

// ---- UpsampleSignal (nearest-neighbour: each sample repeated N times) -------

pub struct UpsampleSignal { pub input: Arc<dyn Signal>, pub factor: usize }

impl Signal for UpsampleSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(UpsampleInstance {
            input: self.input.instantiate(sr),
            factor: self.factor.max(1),
            count: 0,
            cur_val: 0.0,
        })
    }
}

struct UpsampleInstance { input: Box<dyn SignalInstance>, factor: usize, count: usize, cur_val: f32 }

impl SignalInstance for UpsampleInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let mut temp = [0.0f32; 1];
        for s in out.iter_mut() {
            if self.count == 0 {
                self.input.fill(&mut temp);
                self.cur_val = temp[0];
            }
            *s = self.cur_val;
            self.count = (self.count + 1) % self.factor;
        }
    }
}

// ---- DownsampleSignal (nearest-neighbour: take first of every N input samples)

pub struct DownsampleSignal { pub input: Arc<dyn Signal>, pub factor: usize }

impl Signal for DownsampleSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let factor = self.factor.max(1);
        Box::new(DownsampleInstance { input: self.input.instantiate(sr), temp: vec![0.0f32; factor] })
    }
}

struct DownsampleInstance { input: Box<dyn SignalInstance>, temp: Vec<f32> }

impl SignalInstance for DownsampleInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            self.input.fill(&mut self.temp);
            *s = self.temp[0];
        }
    }
}

// ---- DispersalSignal (cascaded 1st-order allpass — frequency-phase dispersion)

pub struct DispersalSignal {
    pub input: Arc<dyn Signal>,
    pub stages: usize,
    pub lo_hz: f32,
    pub hi_hz: f32,
}

impl Signal for DispersalSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let stages = self.stages.max(1);
        let coeffs: Vec<f32> = (0..stages).map(|k| {
            let t = if stages == 1 { 0.5 } else { k as f64 / (stages - 1) as f64 };
            let fc = self.lo_hz as f64 * (self.hi_hz as f64 / (self.lo_hz as f64).max(1e-6)).powf(t);
            let w = (std::f64::consts::PI * fc / sr).tan();
            ((w - 1.0) / (w + 1.0)) as f32  // 1st-order allpass coeff at fc
        }).collect();
        let n = coeffs.len();
        Box::new(DispersalInstance {
            input: self.input.instantiate(sr),
            coeffs,
            x_prev: vec![0.0f32; n],
            y_prev: vec![0.0f32; n],
        })
    }
}

struct DispersalInstance {
    input: Box<dyn SignalInstance>,
    coeffs: Vec<f32>,
    x_prev: Vec<f32>,
    y_prev: Vec<f32>,
}

impl SignalInstance for DispersalInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            let mut x = *s;
            for i in 0..self.coeffs.len() {
                // y[n] = a*x[n] + x[n-1] - a*y[n-1]   (1st-order allpass)
                let a = self.coeffs[i];
                let y = a * x + self.x_prev[i] - a * self.y_prev[i];
                self.x_prev[i] = x;
                self.y_prev[i] = y;
                x = y;
            }
            *s = x;
        }
    }
}

// ---- Strange attractors (RK4 integration) -----------------------------------
// dt is the virtual time step per audio sample.
// Audio-rate chaos: dt ≈ 0.005–0.01. LFO-rate: dt ≈ 0.0005–0.002.

// Lorenz: dx/dt = sigma*(y-x), dy/dt = x*(rho-z)-y, dz/dt = x*y-beta*z
// Classic: sigma=10, rho=28, beta=2.667, dt=0.005. x/y range ≈ ±20, z ∈ [0,50].
pub struct LorenzSignal {
    pub sigma: f32, pub rho: f32, pub beta: f32,
    pub dt: f32,
    pub x0: f32, pub y0: f32, pub z0: f32,
    pub output: u8,   // 0=x, 1=y, 2=z
}

impl Signal for LorenzSignal {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(LorenzInstance {
            sigma: self.sigma, rho: self.rho, beta: self.beta,
            dt: self.dt, x: self.x0, y: self.y0, z: self.z0, output: self.output,
        })
    }
}

struct LorenzInstance { sigma: f32, rho: f32, beta: f32, dt: f32, x: f32, y: f32, z: f32, output: u8 }

impl SignalInstance for LorenzInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let (sigma, rho, beta, dt) = (self.sigma, self.rho, self.beta, self.dt);
        let ld = |x: f32, y: f32, z: f32| -> (f32, f32, f32) {
            (sigma*(y-x), x*(rho-z)-y, x*y-beta*z)
        };
        for s in out.iter_mut() {
            *s = match self.output { 0 => self.x, 1 => self.y, _ => self.z };
            let (k1x,k1y,k1z) = ld(self.x, self.y, self.z);
            let (k2x,k2y,k2z) = ld(self.x+0.5*dt*k1x, self.y+0.5*dt*k1y, self.z+0.5*dt*k1z);
            let (k3x,k3y,k3z) = ld(self.x+0.5*dt*k2x, self.y+0.5*dt*k2y, self.z+0.5*dt*k2z);
            let (k4x,k4y,k4z) = ld(self.x+dt*k3x, self.y+dt*k3y, self.z+dt*k3z);
            self.x += dt/6.0*(k1x+2.0*k2x+2.0*k3x+k4x);
            self.y += dt/6.0*(k1y+2.0*k2y+2.0*k3y+k4y);
            self.z += dt/6.0*(k1z+2.0*k2z+2.0*k3z+k4z);
        }
    }
}

// Rössler: dx/dt = -(y+z), dy/dt = x+a*y, dz/dt = b+z*(x-c)
// Classic: a=0.2, b=0.2, c=5.7, dt=0.01. x range ≈ ±10.
pub struct RosslerSignal {
    pub a: f32, pub b: f32, pub c: f32,
    pub dt: f32,
    pub x0: f32, pub y0: f32, pub z0: f32,
    pub output: u8,
}

impl Signal for RosslerSignal {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(RosslerInstance {
            a: self.a, b: self.b, c: self.c,
            dt: self.dt, x: self.x0, y: self.y0, z: self.z0, output: self.output,
        })
    }
}

struct RosslerInstance { a: f32, b: f32, c: f32, dt: f32, x: f32, y: f32, z: f32, output: u8 }

impl SignalInstance for RosslerInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let (a, b, c, dt) = (self.a, self.b, self.c, self.dt);
        let rd = |x: f32, y: f32, z: f32| -> (f32, f32, f32) {
            (-(y+z), x+a*y, b+z*(x-c))
        };
        for s in out.iter_mut() {
            *s = match self.output { 0 => self.x, 1 => self.y, _ => self.z };
            let (k1x,k1y,k1z) = rd(self.x, self.y, self.z);
            let (k2x,k2y,k2z) = rd(self.x+0.5*dt*k1x, self.y+0.5*dt*k1y, self.z+0.5*dt*k1z);
            let (k3x,k3y,k3z) = rd(self.x+0.5*dt*k2x, self.y+0.5*dt*k2y, self.z+0.5*dt*k2z);
            let (k4x,k4y,k4z) = rd(self.x+dt*k3x, self.y+dt*k3y, self.z+dt*k3z);
            self.x += dt/6.0*(k1x+2.0*k2x+2.0*k3x+k4x);
            self.y += dt/6.0*(k1y+2.0*k2y+2.0*k3y+k4y);
            self.z += dt/6.0*(k1z+2.0*k2z+2.0*k3z+k4z);
        }
    }
}

// Duffing: x'' + delta*x' + alpha*x + beta*x^3 = gamma*cos(omega*t)
// [dx/dt = v,  dv/dt = -delta*v - alpha*x - beta*x^3 + gamma*cos(omega*t)]
// Classic chaotic: alpha=-1, beta=1, delta=0.3, gamma=0.5, omega=1.2, dt=0.1. x range ≈ ±1.
pub struct DuffingSignal {
    pub alpha: f32, pub beta: f32, pub delta: f32,
    pub gamma: f32, pub omega: f32,
    pub dt: f32,
    pub x0: f32, pub v0: f32,
}

impl Signal for DuffingSignal {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(DuffingInstance {
            alpha: self.alpha, beta: self.beta, delta: self.delta,
            gamma: self.gamma, omega: self.omega,
            dt: self.dt, x: self.x0, v: self.v0, t: 0.0,
        })
    }
}

struct DuffingInstance {
    alpha: f32, beta: f32, delta: f32,
    gamma: f32, omega: f32,
    dt: f32, x: f32, v: f32, t: f32,
}

impl SignalInstance for DuffingInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let (alpha, beta, delta, gamma, omega, dt) =
            (self.alpha, self.beta, self.delta, self.gamma, self.omega, self.dt);
        let f = |x: f32, v: f32, t: f32| -> (f32, f32) {
            (v, -delta*v - alpha*x - beta*x*x*x + gamma*(omega*t).cos())
        };
        for s in out.iter_mut() {
            *s = self.x;
            let (k1x, k1v) = f(self.x, self.v, self.t);
            let (k2x, k2v) = f(self.x+0.5*dt*k1x, self.v+0.5*dt*k1v, self.t+0.5*dt);
            let (k3x, k3v) = f(self.x+0.5*dt*k2x, self.v+0.5*dt*k2v, self.t+0.5*dt);
            let (k4x, k4v) = f(self.x+dt*k3x, self.v+dt*k3v, self.t+dt);
            self.x += dt/6.0*(k1x+2.0*k2x+2.0*k3x+k4x);
            self.v += dt/6.0*(k1v+2.0*k2v+2.0*k3v+k4v);
            self.t += dt;
        }
    }
}

// Van der Pol: x'' - mu*(1-x^2)*x' + x = 0
// [dx/dt = v,  dv/dt = mu*(1-x^2)*v - x]
// mu=0: harmonic. mu=1: mild nonlinear. mu>5: relaxation oscillation. x range ≈ ±2.
pub struct VanDerPolSignal {
    pub mu: f32,
    pub dt: f32,
    pub x0: f32, pub v0: f32,
}

impl Signal for VanDerPolSignal {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(VanDerPolInstance { mu: self.mu, dt: self.dt, x: self.x0, v: self.v0 })
    }
}

struct VanDerPolInstance { mu: f32, dt: f32, x: f32, v: f32 }

impl SignalInstance for VanDerPolInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let (mu, dt) = (self.mu, self.dt);
        let f = |x: f32, v: f32| -> (f32, f32) { (v, mu*(1.0-x*x)*v - x) };
        for s in out.iter_mut() {
            *s = self.x;
            let (k1x, k1v) = f(self.x, self.v);
            let (k2x, k2v) = f(self.x+0.5*dt*k1x, self.v+0.5*dt*k1v);
            let (k3x, k3v) = f(self.x+0.5*dt*k2x, self.v+0.5*dt*k2v);
            let (k4x, k4v) = f(self.x+dt*k3x, self.v+dt*k3v);
            self.x += dt/6.0*(k1x+2.0*k2x+2.0*k3x+k4x);
            self.v += dt/6.0*(k1v+2.0*k2v+2.0*k3v+k4v);
        }
    }
}

// ---- VecSignal (array-backed, unchanged) ------------------------------------

/// A finite signal backed by a fixed Vec of f32 samples.
pub struct VecSignal(pub Vec<f32>);

impl Signal for VecSignal {
    fn instantiate(&self, _sr: f64) -> Box<dyn SignalInstance> {
        Box::new(VecSignalInstance { samples: self.0.clone().into(), pos: 0 })
    }
    fn len_hint(&self) -> Option<usize> { Some(self.0.len()) }
    fn as_f32_slice(&self) -> Option<&[f32]> { Some(&self.0) }
}

struct VecSignalInstance {
    samples: Box<[f32]>,
    pos: usize,
}

impl SignalInstance for VecSignalInstance {
    fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = if self.pos < self.samples.len() {
                let v = self.samples[self.pos];
                self.pos += 1;
                v
            } else {
                0.0
            };
        }
    }
}

// ---- SVF (State-Variable Filter, Chamberlin topology) -----------------------
// Simultaneous LP/HP/BP/notch from one filter state. mode: 0=LP 1=HP 2=BP 3=notch

pub struct SvfFilter {
    pub input: Arc<dyn Signal>,
    pub freq_hz: f32,
    pub q: f32,
    pub mode: u8,
}

impl Signal for SvfFilter {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let f = (2.0 * (std::f32::consts::PI * self.freq_hz / sr as f32).sin()).min(1.99);
        let q_inv = (1.0 / self.q.max(0.1)).clamp(0.0, 2.0);
        Box::new(SvfInstance { input: self.input.instantiate(sr), f, q: q_inv, mode: self.mode, lp: 0.0, bp: 0.0 })
    }
}

struct SvfInstance { input: Box<dyn SignalInstance>, f: f32, q: f32, mode: u8, lp: f32, bp: f32 }

impl SignalInstance for SvfInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            let hp = *s - self.lp - self.q * self.bp;
            self.bp += self.f * hp;
            self.lp += self.f * self.bp;
            let notch = *s - self.q * self.bp;
            *s = match self.mode { 0 => self.lp, 1 => hp, 2 => self.bp, _ => notch };
        }
    }
}

// ---- Compressor / limiter ---------------------------------------------------

pub struct CompressorSignal {
    pub input: Arc<dyn Signal>,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_secs: f32,
    pub release_secs: f32,
    pub makeup_db: f32,
}

impl Signal for CompressorSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let att = if self.attack_secs  > 0.0 { (-1.0/(self.attack_secs  as f64 * sr)).exp() as f32 } else { 0.0 };
        let rel = if self.release_secs > 0.0 { (-1.0/(self.release_secs as f64 * sr)).exp() as f32 } else { 0.0 };
        Box::new(CompressorInstance {
            input: self.input.instantiate(sr),
            threshold_db: self.threshold_db, slope: 1.0 - 1.0 / self.ratio.max(1.0),
            att, rel, makeup: self.makeup_db, env: 0.0,
        })
    }
}

struct CompressorInstance {
    input: Box<dyn SignalInstance>,
    threshold_db: f32, slope: f32, att: f32, rel: f32, makeup: f32, env: f32,
}

impl SignalInstance for CompressorInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        for s in out.iter_mut() {
            let abs = s.abs();
            let c = if abs > self.env { self.att } else { self.rel };
            self.env = self.env * c + abs * (1.0 - c);
            let env_db = if self.env > 1e-6 { 20.0 * self.env.log10() } else { -120.0 };
            let gain_db = (if env_db > self.threshold_db { -self.slope*(env_db - self.threshold_db) } else { 0.0 }) + self.makeup;
            *s *= 10.0_f32.powf(gain_db / 20.0);
        }
    }
}

// ---- Window functions -------------------------------------------------------

pub fn hann_window(n: usize) -> Vec<f32> {
    if n == 0 { return vec![]; }
    let n1 = (n - 1).max(1) as f64;
    (0..n).map(|i| (0.5 * (1.0 - (std::f64::consts::TAU * i as f64 / n1).cos())) as f32).collect()
}

pub fn hamming_window(n: usize) -> Vec<f32> {
    if n == 0 { return vec![]; }
    let n1 = (n - 1).max(1) as f64;
    (0..n).map(|i| (0.54 - 0.46 * (std::f64::consts::TAU * i as f64 / n1).cos()) as f32).collect()
}

pub fn blackman_window(n: usize) -> Vec<f32> {
    if n == 0 { return vec![]; }
    let n1 = (n - 1).max(1) as f64;
    (0..n).map(|i| {
        let t = std::f64::consts::TAU * i as f64 / n1;
        (0.42 - 0.5*t.cos() + 0.08*(2.0*t).cos()) as f32
    }).collect()
}

pub fn blackman_harris_window(n: usize) -> Vec<f32> {
    if n == 0 { return vec![]; }
    let n1 = (n - 1).max(1) as f64;
    (0..n).map(|i| {
        let p = std::f64::consts::TAU * i as f64 / n1;
        (0.35875 - 0.48829*p.cos() + 0.14128*(2.0*p).cos() - 0.01168*(3.0*p).cos()) as f32
    }).collect()
}

pub fn nuttall_window(n: usize) -> Vec<f32> {
    if n == 0 { return vec![]; }
    let n1 = (n - 1).max(1) as f64;
    (0..n).map(|i| {
        let p = std::f64::consts::TAU * i as f64 / n1;
        (0.355768 - 0.487396*p.cos() + 0.144232*(2.0*p).cos() - 0.012604*(3.0*p).cos()) as f32
    }).collect()
}

pub fn flat_top_window(n: usize) -> Vec<f32> {
    if n == 0 { return vec![]; }
    let n1 = (n - 1).max(1) as f64;
    (0..n).map(|i| {
        let p = std::f64::consts::TAU * i as f64 / n1;
        (0.21557895 - 0.41663158*p.cos() + 0.277263158*(2.0*p).cos()
            - 0.083578947*(3.0*p).cos() + 0.006947368*(4.0*p).cos()) as f32
    }).collect()
}

pub fn gaussian_window(n: usize, sigma: f64) -> Vec<f32> {
    if n == 0 { return vec![]; }
    let center = (n - 1) as f64 / 2.0;
    (0..n).map(|i| {
        let x = if center == 0.0 { 0.0 } else { (i as f64 - center) / (sigma * center) };
        (-0.5 * x * x).exp() as f32
    }).collect()
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0f64; let mut term = 1.0f64;
    let xh = x / 2.0;
    for k in 1u32..=40 {
        term *= (xh / k as f64).powi(2);
        sum += term;
        if term.abs() < 1e-15 * sum.abs() { break; }
    }
    sum
}

pub fn kaiser_window(n: usize, beta: f64) -> Vec<f32> {
    if n == 0 { return vec![]; }
    let i0b = bessel_i0(beta);
    let center = (n - 1) as f64 / 2.0;
    (0..n).map(|i| {
        let x = if center == 0.0 { 0.0 } else { (i as f64 - center) / center };
        (bessel_i0(beta * (1.0 - x*x).max(0.0).sqrt()) / i0b) as f32
    }).collect()
}

// ---- Hilbert FIR (type-III linear-phase 90° shift) -------------------------

pub struct HilbertFilter { pub input: Arc<dyn Signal> }

pub fn hilbert_coeffs() -> Vec<f32> {
    let n = 63usize;
    let w = blackman_harris_window(n);
    let c = (n / 2) as f64;
    (0..n).map(|k| {
        let m = k as f64 - c;
        if m == 0.0 { 0.0 }
        else if (m as i64).abs() % 2 == 1 { (2.0 / (std::f64::consts::PI * m) * w[k] as f64) as f32 }
        else { 0.0 }
    }).collect()
}

// Shared FIR convolution — used by Hilbert and windowed-sinc filters.
struct FirInstance {
    input: Box<dyn SignalInstance>,
    coeffs: Vec<f32>,
    buffer: Vec<f32>,
    pos: usize,
}

impl SignalInstance for FirInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        let n = self.coeffs.len();
        if n == 0 { return; }
        for s in out.iter_mut() {
            self.buffer[self.pos] = *s;
            let mut y = 0.0f32;
            for k in 0..n { y += self.coeffs[k] * self.buffer[(self.pos + n - k) % n]; }
            self.pos = (self.pos + 1) % n;
            *s = y;
        }
    }
}

impl Signal for HilbertFilter {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let coeffs = hilbert_coeffs();
        let n = coeffs.len();
        Box::new(FirInstance { input: self.input.instantiate(sr), coeffs, buffer: vec![0.0f32; n], pos: 0 })
    }
}

// ---- Windowed-sinc FIR design -----------------------------------------------

pub struct FirFilterSignal { pub input: Arc<dyn Signal>, pub coeffs: Vec<f32> }

impl Signal for FirFilterSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let n = self.coeffs.len();
        Box::new(FirInstance { input: self.input.instantiate(sr), coeffs: self.coeffs.clone(), buffer: vec![0.0f32; n.max(1)], pos: 0 })
    }
}

pub fn fir_coeffs_lp(cutoff_hz: f64, sr: f64, n_taps: usize) -> Vec<f32> {
    let fc = cutoff_hz / sr;
    let center = (n_taps - 1) as f64 / 2.0;
    let w = blackman_harris_window(n_taps);
    (0..n_taps).map(|i| {
        let m = i as f64 - center;
        let sinc = if m == 0.0 { 2.0*fc } else { (std::f64::consts::TAU*fc*m).sin() / (std::f64::consts::PI*m) };
        (sinc * w[i] as f64) as f32
    }).collect()
}

pub fn fir_coeffs_hp(cutoff_hz: f64, sr: f64, n_taps: usize) -> Vec<f32> {
    let mut c = fir_coeffs_lp(cutoff_hz, sr, n_taps);
    let ctr = n_taps / 2;
    for v in c.iter_mut() { *v = -*v; }
    if ctr < c.len() { c[ctr] += 1.0; }
    c
}

pub fn fir_coeffs_bp(lo_hz: f64, hi_hz: f64, sr: f64, n_taps: usize) -> Vec<f32> {
    let lp_hi = fir_coeffs_lp(hi_hz, sr, n_taps);
    let lp_lo = fir_coeffs_lp(lo_hz, sr, n_taps);
    lp_hi.iter().zip(lp_lo.iter()).map(|(a, b)| a - b).collect()
}

// ---- FDN Reverb (Jot / Hadamard feedback delay network) --------------------

pub struct FdnReverb {
    pub input: Arc<dyn Signal>,
    pub n_lines: usize,
    pub decay_secs: f32,
    pub room_size: f32,
}

fn hadamard_in_place(x: &mut [f32]) {
    let n = x.len();
    let norm = (n as f32).sqrt().recip();
    let mut h = 1usize;
    while h < n {
        let mut i = 0;
        while i < n {
            for j in i..i + h {
                let a = x[j]; let b = x[j + h];
                x[j]     = (a + b) * norm;
                x[j + h] = (a - b) * norm;
            }
            i += h * 2;
        }
        h *= 2;
    }
}

impl Signal for FdnReverb {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let n = self.n_lines.next_power_of_two().clamp(2, 16);
        let base_ms: &[f32] = &[29.7, 37.1, 41.1, 53.5, 59.3, 67.7, 73.9, 83.3,
                                  89.1, 97.0, 101.3, 113.9, 127.1, 131.3, 137.7, 149.1];
        let delays: Vec<usize> = base_ms[..n].iter()
            .map(|&ms| ((ms * self.room_size * sr as f32 / 1000.0).round() as usize).max(2))
            .collect();
        let max_d = *delays.iter().max().unwrap();
        let g: Vec<f32> = delays.iter().map(|&d| {
            if self.decay_secs > 0.0 { 10.0_f32.powf(-3.0*d as f32 / (sr as f32*self.decay_secs)) } else { 0.0 }
        }).collect();
        Box::new(FdnInstance {
            input: self.input.instantiate(sr), n, delays, g,
            buffers: vec![vec![0.0f32; max_d + 1]; n],
            write_pos: 0,
            state: vec![0.0f32; n], temp: vec![0.0f32; n],
        })
    }
}

struct FdnInstance {
    input: Box<dyn SignalInstance>, n: usize,
    delays: Vec<usize>, g: Vec<f32>,
    buffers: Vec<Vec<f32>>, write_pos: usize,
    state: Vec<f32>, temp: Vec<f32>,
}

impl SignalInstance for FdnInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        let n = self.n;
        for s in out.iter_mut() {
            let x_in = *s;
            for j in 0..n {
                let blen = self.buffers[j].len();
                let rpos = (self.write_pos + blen - self.delays[j]) % blen;
                self.state[j] = self.buffers[j][rpos] * self.g[j];
            }
            self.temp.copy_from_slice(&self.state);
            hadamard_in_place(&mut self.temp);
            for j in 0..n {
                let blen = self.buffers[j].len();
                self.buffers[j][self.write_pos % blen] = self.temp[j] + x_in;
            }
            self.write_pos += 1;
            *s = self.state.iter().sum::<f32>() / n as f32 * 0.7 + x_in * 0.3;
        }
    }
}

// ---- Waveshaping ------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum WaveShapeMode { Tanh, SoftClip, HardClip, Cubic, Atan, Chebyshev(u8) }

pub struct WaveShaperSignal { pub input: Arc<dyn Signal>, pub mode: WaveShapeMode, pub amount: f32 }

fn chebychev_eval(order: u8, x: f32) -> f32 {
    let x = x.clamp(-1.0, 1.0);
    if order == 0 { return 1.0; }
    let mut t_prev = 1.0f32; let mut t_cur = x;
    if order == 1 { return t_cur; }
    for _ in 2..=order {
        let t_next = 2.0*x*t_cur - t_prev;
        t_prev = t_cur; t_cur = t_next;
    }
    t_cur
}

impl Signal for WaveShaperSignal {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        Box::new(WaveShaperInstance { input: self.input.instantiate(sr), mode: self.mode, amount: self.amount })
    }
}

struct WaveShaperInstance { input: Box<dyn SignalInstance>, mode: WaveShapeMode, amount: f32 }

impl SignalInstance for WaveShaperInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        let a = self.amount;
        for s in out.iter_mut() {
            *s = match self.mode {
                WaveShapeMode::Tanh      => (a * *s).tanh(),
                WaveShapeMode::SoftClip  => { let x = *s * a; x / (1.0 + x.abs()) }
                WaveShapeMode::HardClip  => (*s * a).clamp(-1.0, 1.0),
                WaveShapeMode::Cubic     => { let x = (*s * a).clamp(-1.0, 1.0); x - x*x*x/3.0 }
                WaveShapeMode::Atan      => (*s * a).atan() / std::f32::consts::FRAC_PI_2,
                WaveShapeMode::Chebyshev(order) => chebychev_eval(order, *s * a.min(1.0)).clamp(-1.0, 1.0),
            };
        }
    }
}

// ---- Phase Vocoder (offline: time-stretch and pitch-shift) ------------------

pub fn pvoc_stretch(samples: &[f32], fft_size: usize, hop_in: usize, stretch: f32) -> Vec<f32> {
    use realfft::RealFftPlanner;
    use num_complex::Complex;
    let n = samples.len();
    if n == 0 || fft_size < 4 { return vec![]; }
    let fft_n = fft_size.next_power_of_two();
    let hop_out = ((hop_in as f32 * stretch).round() as usize).max(1);
    let n_frames = (n + hop_in - 1) / hop_in;
    let out_len = n_frames * hop_out + fft_n;

    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_n);
    let c2r = planner.plan_fft_inverse(fft_n);
    let bin_count = fft_n / 2 + 1;
    let window = hann_window(fft_n);

    let mut output = vec![0.0f32; out_len];
    let mut phase_acc  = vec![0.0f32; bin_count];
    let mut prev_phase = vec![0.0f32; bin_count];

    for frame in 0..n_frames {
        let in_start = frame * hop_in;
        let mut frame_buf: Vec<f32> = (0..fft_n).map(|k| {
            let idx = in_start + k;
            if idx < n { samples[idx] * window[k] } else { 0.0 }
        }).collect();
        let mut spectrum = r2c.make_output_vec();
        let _ = r2c.process(&mut frame_buf, &mut spectrum);

        let omega = std::f32::consts::TAU / fft_n as f32;
        for b in 0..bin_count {
            let phase = spectrum[b].arg();
            let dp = phase - prev_phase[b] - omega * b as f32 * hop_in as f32;
            let dp_wrap = dp - std::f32::consts::TAU * (dp / std::f32::consts::TAU).round();
            phase_acc[b] += (omega * b as f32 + dp_wrap / hop_in as f32) * hop_out as f32;
            prev_phase[b] = phase;
            let mag = spectrum[b].norm();
            spectrum[b] = Complex::from_polar(mag, phase_acc[b]);
        }

        let mut out_frame = c2r.make_output_vec();
        let _ = c2r.process(&mut spectrum, &mut out_frame);
        let scale = 2.0 / (fft_n as f32 * hop_in as f32 / hop_out as f32);
        let out_start = frame * hop_out;
        for k in 0..fft_n {
            let idx = out_start + k;
            if idx < output.len() {
                output[idx] += out_frame[k] * window[k] * scale / fft_n as f32;
            }
        }
    }
    let out_samples = ((n as f32 * stretch).round() as usize).min(output.len());
    output.truncate(out_samples);
    output
}

pub fn pvoc_pitch(samples: &[f32], fft_size: usize, hop: usize, semitones: f32) -> Vec<f32> {
    let ratio = 2.0_f32.powf(semitones / 12.0);
    let stretched = pvoc_stretch(samples, fft_size, hop, ratio);
    let out_len = samples.len();
    let src_len = stretched.len();
    if src_len == 0 || out_len == 0 { return vec![0.0; out_len]; }
    (0..out_len).map(|i| {
        let sp = i as f32 * src_len as f32 / out_len as f32;
        let si = sp as usize;
        let frac = sp - si as f32;
        stretched.get(si).copied().unwrap_or(0.0)
            + frac * (stretched.get(si + 1).copied().unwrap_or(0.0) - stretched.get(si).copied().unwrap_or(0.0))
    }).collect()
}

// ---- Granular synthesis -----------------------------------------------------

pub struct GranularSynth {
    pub source: Arc<VecSignal>,
    pub grain_dur_secs: f32,
    pub density: f32,
    pub position: f32,
    pub position_spread: f32,
    pub pitch: f32,
    pub pitch_spread: f32,
    pub seed: u64,
}

struct GrainState { pos: f32, rate: f32, total: usize, env_pos: usize }

struct GranularInstance {
    source: Vec<f32>, grain_samps: usize, trigger_interval: usize,
    position: f32, position_spread: f32, pitch: f32, pitch_spread: f32,
    seed: u64, grains: Vec<GrainState>, countdown: usize,
}

impl Signal for GranularSynth {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let grain_samps = ((self.grain_dur_secs as f64 * sr) as usize).max(2);
        let trigger_interval = ((sr / self.density.max(0.01) as f64) as usize).max(1);
        Box::new(GranularInstance {
            source: self.source.0.clone(), grain_samps, trigger_interval,
            position: self.position.clamp(0.0, 1.0), position_spread: self.position_spread,
            pitch: self.pitch, pitch_spread: self.pitch_spread,
            seed: self.seed, grains: Vec::new(), countdown: 0,
        })
    }
}

impl SignalInstance for GranularInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let src_len = self.source.len();
        if src_len == 0 { out.fill(0.0); return; }
        for s in out.iter_mut() {
            if self.countdown == 0 {
                let r1 = lcg_next(&mut self.seed);
                let pos = (self.position + r1 * 0.5 * self.position_spread).clamp(0.0, 1.0);
                let r2 = lcg_next(&mut self.seed);
                let rate = self.pitch * 2.0_f32.powf(r2 * self.pitch_spread / 12.0);
                self.grains.push(GrainState { pos: pos * src_len as f32, rate, total: self.grain_samps, env_pos: 0 });
                self.countdown = self.trigger_interval;
            } else { self.countdown -= 1; }

            let mut mix = 0.0f32;
            let mut gi = 0;
            while gi < self.grains.len() {
                let pi     = self.grains[gi].pos as usize;
                let frac   = self.grains[gi].pos - pi as f32;
                let env_p  = self.grains[gi].env_pos;
                let total  = self.grains[gi].total;
                let rate   = self.grains[gi].rate;
                let env = (std::f32::consts::PI * env_p as f32 / total.max(1) as f32).sin().powi(2);
                let a = self.source.get(pi).copied().unwrap_or(0.0);
                let b = self.source.get(pi + 1).copied().unwrap_or(0.0);
                mix += (a + frac * (b - a)) * env;
                self.grains[gi].pos += rate;
                self.grains[gi].env_pos += 1;
                if self.grains[gi].env_pos >= self.grains[gi].total {
                    self.grains.swap_remove(gi);
                } else { gi += 1; }
            }
            *s = mix;
        }
    }
}

// ---- LPC (Levinson-Durbin) --------------------------------------------------

pub fn lpc_analyze(signal: &[f32], order: usize) -> Vec<f32> {
    let n = signal.len();
    if n == 0 || order == 0 { return vec![0.0; order]; }
    let r: Vec<f64> = (0..=order).map(|lag| {
        (0..n.saturating_sub(lag)).map(|i| signal[i] as f64 * signal[i + lag] as f64).sum::<f64>()
    }).collect();
    let mut a = vec![0.0f64; order + 1];
    a[0] = 1.0;
    let mut err = r[0];
    for i in 1..=order {
        let num: f64 = (0..i).map(|j| a[j] * r[i - j]).sum::<f64>();
        let k = if err.abs() < 1e-15 { 0.0 } else { -num / err };
        let mut a2 = a.clone();
        for j in 1..i { a2[j] = a[j] + k * a[i - j]; }
        a2[i] = k;
        a = a2;
        err *= 1.0 - k * k;
    }
    a[1..=order].iter().map(|&x| x as f32).collect()
}

pub fn lpc_synthesize(excitation: &[f32], coeffs: &[f32]) -> Vec<f32> {
    let order = coeffs.len();
    let mut out = vec![0.0f32; excitation.len()];
    for n in 0..excitation.len() {
        let fb: f32 = (1..=order.min(n)).map(|k| -coeffs[k-1] * out[n-k]).sum();
        out[n] = excitation[n] + fb;
    }
    out
}

// ---- Goertzel (single-frequency DFT) ----------------------------------------

pub fn goertzel_magnitude(samples: &[f32], freq_hz: f64, sr: f64) -> f32 {
    let n = samples.len();
    if n == 0 { return 0.0; }
    let omega = std::f64::consts::TAU * freq_hz / sr;
    let coeff = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in samples {
        let s0 = coeff * s1 - s2 + x as f64;
        s2 = s1; s1 = s0;
    }
    let re = s1 - s2 * omega.cos();
    let im = s2 * omega.sin();
    ((re*re + im*im).sqrt() / n as f64) as f32
}

pub fn goertzel_complex(samples: &[f32], freq_hz: f64, sr: f64) -> (f32, f32) {
    let n = samples.len();
    if n == 0 { return (0.0, 0.0); }
    let omega = std::f64::consts::TAU * freq_hz / sr;
    let coeff = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in samples {
        let s0 = coeff * s1 - s2 + x as f64;
        s2 = s1; s1 = s0;
    }
    ((( s1 - s2*omega.cos()) / n as f64) as f32,
     ((s2 * omega.sin()) / n as f64) as f32)
}

// ---- MDCT / IMDCT -----------------------------------------------------------

pub fn mdct(samples: &[f32]) -> Vec<f32> {
    let n = samples.len();
    let m = n / 2;
    if m == 0 { return vec![]; }
    let scale = (2.0 / n as f64).sqrt();
    (0..m).map(|k| {
        let sum: f64 = (0..n).map(|i| {
            let a = std::f64::consts::PI / n as f64 * (i as f64 + 0.5 + m as f64 / 2.0) * (k as f64 + 0.5);
            samples[i] as f64 * a.cos()
        }).sum();
        (scale * sum) as f32
    }).collect()
}

pub fn imdct(coeffs: &[f32]) -> Vec<f32> {
    let m = coeffs.len();
    let n = m * 2;
    if n == 0 { return vec![]; }
    let scale = (2.0 / n as f64).sqrt();
    (0..n).map(|i| {
        let sum: f64 = (0..m).map(|k| {
            let a = std::f64::consts::PI / n as f64 * (i as f64 + 0.5 + m as f64 / 2.0) * (k as f64 + 0.5);
            coeffs[k] as f64 * a.cos()
        }).sum();
        (scale * sum) as f32
    }).collect()
}

// ---- Thiran allpass (maximally-flat fractional delay) -----------------------

pub struct ThiranAllpass { pub input: Arc<dyn Signal>, pub delay_samples: f64, pub order: usize }

fn thiran_coeffs(d: f64, order: usize) -> Vec<f64> {
    let mut a = vec![0.0f64; order + 1];
    a[0] = 1.0;
    for k in 1..=order {
        let mut binom = 1usize;
        for j in 0..k { binom = binom * (order - j) / (j + 1); }
        let sign = if k % 2 == 0 { 1.0f64 } else { -1.0 };
        let prod: f64 = (0..=order).map(|i| {
            let num = d - order as f64 + i as f64;
            let den = d - order as f64 + k as f64 + i as f64;
            if den.abs() < 1e-12 { 1.0 } else { num / den }
        }).product();
        a[k] = sign * binom as f64 * prod;
    }
    a
}

impl Signal for ThiranAllpass {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let order = self.order.clamp(1, 8);
        let d = self.delay_samples.max(order as f64 + 0.01);
        let a = thiran_coeffs(d, order);
        let buf_n = order + 1;
        Box::new(ThiranInstance {
            input: self.input.instantiate(sr), a, order,
            x_buf: vec![0.0f32; buf_n], y_buf: vec![0.0f32; buf_n], pos: 0,
        })
    }
}

struct ThiranInstance { input: Box<dyn SignalInstance>, a: Vec<f64>, order: usize, x_buf: Vec<f32>, y_buf: Vec<f32>, pos: usize }

impl SignalInstance for ThiranInstance {
    fn fill(&mut self, out: &mut [f32]) {
        self.input.fill(out);
        let n = self.order + 1;
        for s in out.iter_mut() {
            self.x_buf[self.pos] = *s;
            let mut y = 0.0f64;
            for k in 0..=self.order {
                y += self.a[self.order - k] * self.x_buf[(self.pos + n - k) % n] as f64;
            }
            for k in 1..=self.order {
                y -= self.a[k] * self.y_buf[(self.pos + n - k) % n] as f64;
            }
            let yf = y as f32;
            self.y_buf[self.pos] = yf;
            self.pos = (self.pos + 1) % n;
            *s = yf;
        }
    }
}

// ---- Farrow fractional delay (variable, 3rd-order Lagrange) -----------------

pub struct FarrowDelay {
    pub input: Arc<dyn Signal>,
    pub delay_signal: Arc<dyn Signal>,
    pub max_delay_secs: f32,
}

impl Signal for FarrowDelay {
    fn instantiate(&self, sr: f64) -> Box<dyn SignalInstance> {
        let max_d = ((self.max_delay_secs as f64 * sr) as usize + 4).max(8);
        Box::new(FarrowInstance {
            input: self.input.instantiate(sr),
            delay_mod: self.delay_signal.instantiate(sr),
            buffer: vec![0.0f32; max_d],
            delay_buf: vec![0.0f32; 512],
            pos: 0, max_d,
        })
    }
}

struct FarrowInstance {
    input: Box<dyn SignalInstance>, delay_mod: Box<dyn SignalInstance>,
    buffer: Vec<f32>, delay_buf: Vec<f32>, pos: usize, max_d: usize,
}

impl SignalInstance for FarrowInstance {
    fn fill(&mut self, out: &mut [f32]) {
        let n_out = out.len();
        self.delay_buf.resize(n_out, 0.0);
        self.input.fill(out);
        self.delay_mod.fill(&mut self.delay_buf[..n_out]);
        let blen = self.buffer.len();
        for k in 0..n_out {
            let xn = out[k];
            self.buffer[self.pos] = xn;
            let d = self.delay_buf[k].clamp(2.0, (self.max_d - 3) as f32);
            let di = d as usize;
            let mu = d - di as f32;
            // Catmull-Rom: at mu=0 → p[1] (di samples back), mu=1 → p[2]
            let p = [
                self.buffer[(self.pos + blen - di - 1) % blen],
                self.buffer[(self.pos + blen - di    ) % blen],
                self.buffer[(self.pos + blen - di + 1) % blen],
                self.buffer[(self.pos + blen - di + 2) % blen],
            ];
            let y = p[1] + 0.5 * mu * (p[2] - p[0]
                + mu * (2.0*p[0] - 5.0*p[1] + 4.0*p[2] - p[3]
                + mu * (-p[0] + 3.0*p[1] - 3.0*p[2] + p[3])));
            self.pos = (self.pos + 1) % blen;
            out[k] = y;
        }
    }
}

// ---- CQT (Constant-Q Transform) magnitude spectrum -------------------------

pub fn cqt_magnitudes(samples: &[f32], sr: f64, bins_per_octave: usize, f_min: f64, n_bins: usize) -> Vec<f32> {
    if samples.is_empty() || n_bins == 0 || bins_per_octave == 0 { return vec![]; }
    let q = 1.0 / (2.0_f64.powf(1.0 / bins_per_octave as f64) - 1.0);
    (0..n_bins).map(|k| {
        let f_k = f_min * 2.0_f64.powf(k as f64 / bins_per_octave as f64);
        let n_k = ((q * sr / f_k).round() as usize).min(samples.len()).max(1);
        let window = hann_window(n_k);
        let windowed: Vec<f32> = samples[..n_k].iter().zip(window.iter()).map(|(&x, &w)| x * w).collect();
        goertzel_magnitude(&windowed, f_k, sr)
    }).collect()
}

// ---- FFT (real-to-complex magnitude spectrum) --------------------------------

/// Compute the magnitude spectrum of a finite signal via realfft.
/// Returns a VecSignal of length n/2+1 magnitudes.
pub fn fft_magnitude(samples: &[f32]) -> Vec<f32> {
    use realfft::RealFftPlanner;
    let n = samples.len();
    if n == 0 { return vec![]; }
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(n);
    let mut input = samples.to_vec();
    let mut spectrum = r2c.make_output_vec();
    if r2c.process(&mut input, &mut spectrum).is_err() {
        return vec![0.0; n / 2 + 1];
    }
    spectrum.iter().map(|c| (c.re * c.re + c.im * c.im).sqrt()).collect()
}

/// Reconstruct a signal from a magnitude spectrum (zero-phase IFFT).
pub fn ifft_from_magnitude(mags: &[f32]) -> Vec<f32> {
    use realfft::RealFftPlanner;
    if mags.is_empty() { return vec![]; }
    let n = (mags.len() - 1) * 2;
    if n == 0 { return vec![]; }
    let mut planner = RealFftPlanner::<f32>::new();
    let c2r = planner.plan_fft_inverse(n);
    let mut spectrum: Vec<num_complex::Complex<f32>> = mags.iter()
        .map(|&m| num_complex::Complex::new(m, 0.0))
        .collect();
    let mut output = c2r.make_output_vec();
    if c2r.process(&mut spectrum, &mut output).is_err() {
        return vec![0.0; n];
    }
    let scale = 1.0 / n as f32;
    output.iter().map(|&x| x * scale).collect()
}

// ---- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_runs() {
        let osc = SinOsc::new(440.0);
        let mut inst = osc.instantiate(48_000.0);
        let mut buf = [0.0f32; 64];
        inst.fill(&mut buf);
        assert!(buf[0].abs() < 1e-5);
    }

    #[test]
    fn saw_runs() {
        let osc = SawOsc::new(440.0);
        let mut inst = osc.instantiate(48_000.0);
        let mut buf = [0.0f32; 64];
        inst.fill(&mut buf);
        // First sample at phase 0: output = 0*2-1 = -1
        assert!((buf[0] - (-1.0)).abs() < 1e-5);
    }

    #[test]
    fn white_noise_varies() {
        let n = WhiteNoise::new(42);
        let mut inst = n.instantiate(44100.0);
        let mut buf = [0.0f32; 64];
        inst.fill(&mut buf);
        // All samples should not be the same
        let first = buf[0];
        assert!(buf.iter().any(|&x| (x - first).abs() > 1e-5));
    }

    #[test]
    fn pluck_runs() {
        let p = PluckOsc::new(440.0, 1.0);
        let mut inst = p.instantiate(44100.0);
        let mut buf = [0.0f32; 256];
        inst.fill(&mut buf);
        // Should produce some non-zero output
        assert!(buf.iter().any(|&x| x.abs() > 1e-5));
    }

    #[test]
    fn ar_env_shape() {
        let env = ArEnv { attack_secs: 0.001, release_secs: 0.001 };
        let mut inst = env.instantiate(1000.0);
        let mut buf = [0.0f32; 4];
        inst.fill(&mut buf);
        // attack: rising
        assert!(buf[0] <= buf[1]);
    }

    #[test]
    fn fft_roundtrip() {
        let freq = 440.0f32;
        let sr = 44100.0f32;
        let n = 1024;
        let samples: Vec<f32> = (0..n).map(|i| {
            (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin()
        }).collect();
        let mags = fft_magnitude(&samples);
        assert_eq!(mags.len(), n / 2 + 1);
        // Peak at 440 Hz bin
        let bin = (freq * n as f32 / sr).round() as usize;
        let peak = mags[bin];
        assert!(peak > 200.0, "expected peak at 440Hz bin, got {peak}");
    }
}
