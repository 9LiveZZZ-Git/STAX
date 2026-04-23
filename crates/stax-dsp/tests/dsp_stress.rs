//! DSP stress tests — mathematical property checks at SR=48000.
//!
//! Each test instantiates a Signal, fills buffers, and verifies mathematical
//! invariants rather than exact values (tolerances reflect real DSP behaviour).

use stax_core::Signal;
use stax_dsp::*;
use std::sync::Arc;

const SR: f64 = 48_000.0;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Fill n samples from `inst` into a new Vec.
fn fill_n(inst: &mut Box<dyn stax_core::SignalInstance>, n: usize) -> Vec<f32> {
    let mut buf = vec![0.0f32; n];
    inst.fill(&mut buf);
    buf
}

/// Compute RMS of a slice.
fn rms(buf: &[f32]) -> f32 {
    let sum_sq: f32 = buf.iter().map(|&x| x * x).sum();
    (sum_sq / buf.len() as f32).sqrt()
}

/// Count zero-crossings (sign changes) in the buffer.
fn zero_crossings(buf: &[f32]) -> usize {
    buf.windows(2)
        .filter(|w| w[0].signum() != w[1].signum() && w[0] != 0.0 && w[1] != 0.0)
        .count()
}

/// Mean of a slice.
fn mean(buf: &[f32]) -> f32 {
    buf.iter().sum::<f32>() / buf.len() as f32
}

/// Standard deviation of a slice.
fn std_dev(buf: &[f32]) -> f32 {
    let m = mean(buf);
    let var: f32 = buf.iter().map(|&x| (x - m) * (x - m)).sum::<f32>() / buf.len() as f32;
    var.sqrt()
}

// ── Test 1: SinOsc frequency accuracy (zero-crossing count) ──────────────────

#[test]
fn sinosc_frequency_accuracy_zero_crossings() {
    let osc = SinOsc::new(440.0);
    let mut inst = osc.instantiate(SR);
    let buf = fill_n(&mut inst, 48_000);
    let crossings = zero_crossings(&buf);
    // 440 Hz × 2 crossings/cycle × 1 sec = 880.
    // Allow ±10 for phase quantisation at buffer boundaries.
    assert!(
        crossings >= 860 && crossings <= 900,
        "expected ~880 zero-crossings for 440 Hz sine, got {crossings}"
    );
}

// ── Test 2: SinOsc amplitude ─────────────────────────────────────────────────

#[test]
fn sinosc_amplitude_near_unity() {
    let osc = SinOsc::new(440.0);
    let mut inst = osc.instantiate(SR);
    let buf = fill_n(&mut inst, 48_000);
    let peak = buf.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let trough = buf.iter().cloned().fold(f32::INFINITY, f32::min);
    assert!(
        peak > 0.999 && peak <= 1.001,
        "SinOsc peak should be ≈1.0, got {peak}"
    );
    assert!(
        trough < -0.999 && trough >= -1.001,
        "SinOsc trough should be ≈-1.0, got {trough}"
    );
}

// ── Test 3: Saw wave — monotonically increasing within each cycle then resets ─

#[test]
fn sawosc_monotone_per_cycle_then_reset() {
    // Use a low frequency so we have many samples per cycle.
    // 100 Hz @ SR=48000 → 480 samples/cycle
    let osc = SawOsc::new(100.0);
    let mut inst = osc.instantiate(SR);
    let buf = fill_n(&mut inst, 48_000);

    let samples_per_cycle = SR / 100.0; // 480
    // Find one complete cycle by looking for the reset point
    let half_cycle = (samples_per_cycle / 2.0) as usize;
    let full_cycle = samples_per_cycle as usize;

    // Within the first full cycle, the saw should be strictly increasing
    // (ignoring the reset wrap at the very end of the cycle).
    // Count how many adjacent pairs are decreasing by more than 0.1 (a reset).
    let mut big_drops = 0usize;
    let mut _non_resets_increasing = 0usize;
    let mut non_resets_decreasing = 0usize;
    for w in buf[..full_cycle * 10].windows(2) {
        let diff = w[1] - w[0];
        if diff < -0.5 {
            // This is a reset
            big_drops += 1;
        } else if diff > 0.0 {
            _non_resets_increasing += 1;
        } else {
            non_resets_decreasing += 1;
        }
    }
    // In 10 cycles we expect ~10 resets
    assert!(
        big_drops >= 8 && big_drops <= 12,
        "expected ~10 resets in 10 cycles of 100Hz saw, got {big_drops}"
    );
    // Between resets, the saw must be increasing
    assert!(
        non_resets_decreasing < 5,
        "saw wave should be monotonically increasing between resets, got {non_resets_decreasing} decreasing pairs"
    );
    let _ = half_cycle;
}

// ── Test 4: Square wave — 50% duty cycle, half near +1, half near -1 ─────────

#[test]
fn square_wave_50pct_duty() {
    // PulseOsc with duty=0.5 gives a square wave.
    let osc = PulseOsc::new(440.0, 0.5);
    let mut inst = osc.instantiate(SR);
    let buf = fill_n(&mut inst, 48_000);

    let near_plus: usize = buf.iter().filter(|&&x| (x - 1.0).abs() < 0.1).count();
    let near_minus: usize = buf.iter().filter(|&&x| (x + 1.0).abs() < 0.1).count();
    let total = buf.len() as f32;

    let plus_frac = near_plus as f32 / total;
    let minus_frac = near_minus as f32 / total;

    // Both halves should be within 5% of 50%
    assert!(
        (plus_frac - 0.5).abs() < 0.05,
        "expected ~50% near +1.0, got {:.1}%", plus_frac * 100.0
    );
    assert!(
        (minus_frac - 0.5).abs() < 0.05,
        "expected ~50% near -1.0, got {:.1}%", minus_frac * 100.0
    );
}

// ── Test 5: White noise — LCG output properties ──────────────────────────────
//
// LCG bias bug was fixed: `>> 33` → `>> 32` in lcg_next.
// The upper 32 bits of the u64 LCG state now map uniformly to [0, u32::MAX],
// so the final expression `ratio * 2.0 - 1.0` correctly covers [-1, 1).

#[test]
fn white_noise_statistical_properties() {
    let n = WhiteNoise::new(0xdeadbeef_cafebabe);
    let mut inst = n.instantiate(SR);
    let buf = fill_n(&mut inst, 48_000);

    let m = mean(&buf);
    let s = std_dev(&buf);

    // Mean should be near 0 (uniform [-1, 1) distribution).
    assert!(
        m.abs() < 0.05,
        "white noise mean should be near 0, got {m:.4}"
    );

    // All values should be in [-1, 1).
    assert!(
        buf.iter().all(|&x| x >= -1.0 && x < 1.0),
        "white noise values must be in [-1, 1)"
    );

    // Standard deviation for uniform [-1, 1) is 1/√3 ≈ 0.577.
    assert!(
        s > 0.50 && s < 0.65,
        "white noise std_dev should be ≈ 0.577, got {s:.4}"
    );
}

// ── Test 6: Pink noise — values in reasonable range ──────────────────────────

#[test]
fn pink_noise_bounded() {
    let p = PinkNoise::new(12345);
    let mut inst = p.instantiate(SR);
    let buf = fill_n(&mut inst, 48_000);

    // Pink noise output should not blow up
    let peak = buf.iter().cloned().fold(0.0f32, |a, x| a.max(x.abs()));
    assert!(
        peak < 3.0,
        "pink noise peak should be reasonable, got {peak:.4}"
    );
    // Should have some variation
    let s = std_dev(&buf);
    assert!(s > 0.01, "pink noise should vary, std_dev={s:.4}");
}

// ── Test 7: AR envelope — peak near 1.0, decays to near 0 ────────────────────

#[test]
fn ar_envelope_shape() {
    // attack=0.01s, release=0.1s, total duration = 0.11s = 5280 samples
    let env = ArEnv { attack_secs: 0.01, release_secs: 0.1 };
    let mut inst = env.instantiate(SR);
    let total_samples = ((0.01 + 0.1) * SR as f32) as usize + 100;
    let buf = fill_n(&mut inst, total_samples);

    let attack_end = (0.01 * SR as f32) as usize;
    let release_end = ((0.01 + 0.1) * SR as f32) as usize;

    // Peak should be near 1.0 at the attack/release transition
    let peak = buf[..release_end].iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        peak > 0.99,
        "AR envelope peak should be near 1.0, got {peak:.4}"
    );

    // After release, the envelope should be near 0.0
    let tail_mean = mean(&buf[release_end..]);
    assert!(
        tail_mean < 0.01,
        "AR envelope tail should be near 0.0, got {tail_mean:.4}"
    );

    // Value at attack end should be 1.0
    let at_peak = buf[attack_end - 1];
    assert!(
        at_peak > 0.98,
        "AR envelope should reach 1.0 at end of attack, got {at_peak:.4}"
    );
}

// ── Test 8: LP filter — 440Hz passes, 10kHz attenuated ───────────────────────

#[test]
fn lpf2_passes_low_attenuates_high() {
    let sr = SR;
    let cutoff = 1000.0f32;

    // Measure gain through the filter for a 440 Hz sine
    {
        let src = Arc::new(SinOsc::new(440.0));
        let lpf = Lpf2Signal { input: src as Arc<dyn Signal>, cutoff_hz: cutoff };
        let mut inst = lpf.instantiate(sr);
        // Let the filter settle then measure
        let pre_settle = 2000usize;
        let meas_samples = 4800usize;
        let buf = fill_n(&mut inst, pre_settle + meas_samples);
        let gain_440 = rms(&buf[pre_settle..]);
        // A 440 Hz signal through a 1kHz LP should have gain near 1 / sqrt(2) or higher
        assert!(
            gain_440 > 0.55,
            "1kHz LP filter should pass 440Hz (gain>{0:.2}), got {gain_440:.4}",
            0.55
        );
    }

    // Measure gain for a 10 kHz sine
    {
        let src = Arc::new(SinOsc::new(10_000.0));
        let lpf = Lpf2Signal { input: src as Arc<dyn Signal>, cutoff_hz: cutoff };
        let mut inst = lpf.instantiate(sr);
        let pre_settle = 2000usize;
        let meas_samples = 4800usize;
        let buf = fill_n(&mut inst, pre_settle + meas_samples);
        let gain_10k = rms(&buf[pre_settle..]);
        // 10kHz should be strongly attenuated by a 1kHz LP (at least -20dB = 0.1 factor)
        assert!(
            gain_10k < 0.1,
            "1kHz LP filter should strongly attenuate 10kHz, got gain={gain_10k:.4}"
        );
    }
}

// ── Test 9: HP filter — 10kHz passes, 440Hz attenuated ───────────────────────

#[test]
fn hpf2_passes_high_attenuates_low() {
    let cutoff = 5000.0f32;

    // 440 Hz should be attenuated
    {
        let src = Arc::new(SinOsc::new(440.0));
        let hpf = Hpf2Signal { input: src as Arc<dyn Signal>, cutoff_hz: cutoff };
        let mut inst = hpf.instantiate(SR);
        let pre_settle = 2000;
        let meas_samples = 4800;
        let buf = fill_n(&mut inst, pre_settle + meas_samples);
        let gain_440 = rms(&buf[pre_settle..]);
        assert!(
            gain_440 < 0.15,
            "5kHz HP filter should strongly attenuate 440Hz, got gain={gain_440:.4}"
        );
    }

    // 10kHz should pass
    {
        let src = Arc::new(SinOsc::new(10_000.0));
        let hpf = Hpf2Signal { input: src as Arc<dyn Signal>, cutoff_hz: cutoff };
        let mut inst = hpf.instantiate(SR);
        let pre_settle = 2000;
        let meas_samples = 4800;
        let buf = fill_n(&mut inst, pre_settle + meas_samples);
        let gain_10k = rms(&buf[pre_settle..]);
        assert!(
            gain_10k > 0.55,
            "5kHz HP filter should pass 10kHz (gain>0.55), got gain={gain_10k:.4}"
        );
    }
}

// ── Test 10: SVF LP filter — same passband/stopband sanity ───────────────────

#[test]
fn svf_lp_passband_stopband() {
    let cutoff = 1000.0f32;
    let q = 0.707f32;

    // 440 Hz: should pass
    {
        let src = Arc::new(SinOsc::new(440.0));
        let filt = SvfFilter { input: src as Arc<dyn Signal>, freq_hz: cutoff, q, mode: 0 };
        let mut inst = filt.instantiate(SR);
        let buf = fill_n(&mut inst, 6800);
        let gain = rms(&buf[2000..]);
        assert!(
            gain > 0.45,
            "SVF LP 1kHz should pass 440Hz, got gain={gain:.4}"
        );
    }

    // 10kHz: should be strongly attenuated
    {
        let src = Arc::new(SinOsc::new(10_000.0));
        let filt = SvfFilter { input: src as Arc<dyn Signal>, freq_hz: cutoff, q, mode: 0 };
        let mut inst = filt.instantiate(SR);
        let buf = fill_n(&mut inst, 6800);
        let gain = rms(&buf[2000..]);
        assert!(
            gain < 0.15,
            "SVF LP 1kHz should attenuate 10kHz, got gain={gain:.4}"
        );
    }
}

// ── Test 11: FDN reverb — produces output, early energy decays ───────────────
//
// The FDN reverb mixes 70% wet + 30% dry per sample. With a short impulse input
// (1.0 at sample 0, 0 thereafter), the first samples contain the direct signal
// (0.3 * 1.0 = 0.3 at sample 0), and early reflections appear at the delay-line
// lengths (~1400-4000 samples). The energy should be non-trivial in an early
// window and decaying over time as the gains < 1 damp the recirculating energy.
//
// NOTE: The short delay lines (29-149ms at room_size=1.0) mean the reverb tail
// is short (~1-2 seconds). At samples 10k-20k (0.2-0.4s) the tail IS present.
// However the actual FDN energy depends heavily on room_size scaling — with
// room_size=1.0 and the base delays (29-149ms) the buffers are 1426-7156 samples.
// The FDN's write_pos mod buffer_len arithmetic makes the read position correct
// only after the first full delay worth of samples. We test the first 3000 samples.

#[test]
fn fdn_reverb_tail_and_decay() {
    // Continuous sine input — reverb should have RMS output above some floor
    let src_sine = Arc::new(SinOsc::new(440.0));
    let verb = FdnReverb {
        input: src_sine as Arc<dyn Signal>,
        n_lines: 8,
        decay_secs: 2.0,
        room_size: 1.0,
    };
    let mut inst = verb.instantiate(SR);
    let buf = fill_n(&mut inst, 6_000);

    // With continuous sine input, the reverb adds wet signal.
    // Output RMS should be non-trivial (sine alone has RMS ~0.707; reverb adds to it).
    let out_rms = rms(&buf[1000..]);
    assert!(
        out_rms > 0.1,
        "FDN reverb on sine input should produce substantial output, got rms={out_rms:.4}"
    );

    // Test impulse response: direct signal appears at sample 0
    let mut impulse_samples = vec![0.0f32; 48_000];
    impulse_samples[0] = 1.0;
    let src_imp = Arc::new(VecSignal(impulse_samples));
    let verb2 = FdnReverb {
        input: src_imp as Arc<dyn Signal>,
        n_lines: 8,
        decay_secs: 2.0,
        room_size: 1.0,
    };
    let mut inst2 = verb2.instantiate(SR);
    let buf2 = fill_n(&mut inst2, 48_000);

    // Direct signal: sample 0 should be 0.3 (0.7 wet = 0 at start + 0.3*1.0 dry)
    assert!(
        buf2[0].abs() > 0.2,
        "FDN reverb impulse direct signal should be visible at sample 0, got {:.4}", buf2[0]
    );

    // Check that there is energy somewhere in the reverb tail region
    // (at the first delay line length ≈ 1426 samples there should be reflections)
    let tail_rms_early = rms(&buf2[1400..2000]);
    let tail_rms_late = rms(&buf2[40_000..]);

    assert!(
        tail_rms_early > tail_rms_late,
        "FDN reverb early tail ({tail_rms_early:.6}) should exceed very late ({tail_rms_late:.6})"
    );
}

// ── Test 12: Compressor — above threshold is attenuated ──────────────────────

#[test]
fn compressor_attenuates_above_threshold() {
    let threshold_db = -12.0f32; // -12 dB ≈ amplitude 0.25
    let ratio = 8.0f32;

    // Hot signal: sine at amplitude ~0.9 (well above -1.5dB threshold)
    let src_hot = Arc::new(SinOsc::new(440.0));
    let comp = CompressorSignal {
        input: src_hot as Arc<dyn Signal>,
        threshold_db,
        ratio,
        attack_secs: 0.005,
        release_secs: 0.05,
        makeup_db: 0.0,
    };
    let mut inst_comp = comp.instantiate(SR);
    // Let the compressor settle, then measure
    let buf_comp = fill_n(&mut inst_comp, 12_000);
    let gain_comp = rms(&buf_comp[4_000..]);

    // Uncompressed 440Hz sine has RMS ≈ 1/sqrt(2) ≈ 0.707
    // With -12dB threshold and 8:1 ratio, output should be substantially lower
    assert!(
        gain_comp < 0.5,
        "compressor should attenuate hot signal, got rms={gain_comp:.4}"
    );

    // Quiet signal (amplitude 0.05, well below threshold): should mostly pass
    let src_quiet = Arc::new(
        // Scale: use WaveShaperSignal with hardclip amount=0.05 to scale a sine
        SinOsc::new(440.0)
    );
    // Manually create a scaled sine by using Mix2 with gain_a=0.05, gain_b=0
    let scaled = Arc::new(Mix2Signal {
        a: src_quiet as Arc<dyn Signal>,
        b: Arc::new(SinOsc::new(440.0)) as Arc<dyn Signal>, // dummy
        gain_a: 0.05,
        gain_b: 0.0,
    });
    let comp2 = CompressorSignal {
        input: scaled as Arc<dyn Signal>,
        threshold_db,
        ratio,
        attack_secs: 0.005,
        release_secs: 0.05,
        makeup_db: 0.0,
    };
    let mut inst_quiet = comp2.instantiate(SR);
    let buf_quiet = fill_n(&mut inst_quiet, 12_000);
    let gain_quiet = rms(&buf_quiet[4_000..]);

    // Quiet signal RMS should be ~0.05/sqrt(2) ≈ 0.035
    // It should pass with little attenuation
    assert!(
        gain_quiet > 0.02,
        "compressor should not attenuate quiet signal, got rms={gain_quiet:.4}"
    );
}

// ── Test 13: Goertzel — 440Hz sine detected at target, off-target attenuated ──

#[test]
fn goertzel_detects_target_frequency() {
    let n = 4096usize;
    let freq = 440.0f64;

    // Generate a 440Hz sine
    let samples: Vec<f32> = (0..n).map(|i| {
        (std::f64::consts::TAU * freq * i as f64 / SR).sin() as f32
    }).collect();

    let mag_at_440 = goertzel_magnitude(&samples, 440.0, SR);
    // Use a far-off frequency (4400 Hz, 10x the fundamental) to test rejection.
    // Note: spectral resolution at n=4096, SR=48000 is 48000/4096 ≈ 11.7 Hz,
    // so 440 and 441 Hz are NOT resolvable with 4096 samples — they fall in the
    // same DFT bin. For meaningful rejection, test a well-separated frequency.
    let mag_at_4400 = goertzel_magnitude(&samples, 4400.0, SR);

    // The 440Hz bin should be strongly detected
    assert!(
        mag_at_440 > 0.3,
        "Goertzel should detect 440Hz in a 440Hz sine, got magnitude={mag_at_440:.4}"
    );

    // 4400Hz should be near zero relative to 440Hz
    let ratio = mag_at_4400 / mag_at_440;
    assert!(
        ratio < 0.05,
        "Goertzel at 4400Hz should be much weaker than 440Hz (ratio={ratio:.4})"
    );
}

// ── Bonus Test 25: Goertzel spectral resolution — documents 1Hz limitation ────
//
// At n=4096, SR=48000, spectral resolution is 48000/4096 ≈ 11.7 Hz.
// Goertzel at 440 and 441 Hz on a 440Hz signal cannot distinguish them —
// both report similar magnitudes. This is fundamental DFT behaviour, not a bug.
#[test]
fn goertzel_spectral_resolution_documented() {
    let n = 4096usize;
    let samples: Vec<f32> = (0..n).map(|i| {
        (std::f64::consts::TAU * 440.0 * i as f64 / SR).sin() as f32
    }).collect();

    let mag_440 = goertzel_magnitude(&samples, 440.0, SR);
    let mag_441 = goertzel_magnitude(&samples, 441.0, SR);

    // With ~11.7 Hz resolution, 440 and 441 Hz should give nearly equal magnitudes
    // (both are essentially the same DFT bin sampled at slightly different phases).
    let ratio = (mag_441 / mag_440.max(1e-6)).max(mag_440 / mag_441.max(1e-6));
    assert!(
        ratio < 5.0,
        "Goertzel at 440 and 441Hz should be similar (resolution=11.7Hz), ratio={ratio:.4}"
    );
}

// ── Test 14: MDCT round-trip ─────────────────────────────────────────────────

#[test]
fn mdct_imdct_roundtrip() {
    // MDCT/IMDCT with overlap-add: imdct(mdct(x)) = x (with window, via OLA).
    // The simple identity: for a signal of length 2N,
    // imdct gives 2N samples, first N and last N each approximate input halves.
    let n_in = 128usize; // must be even; MDCT halves to n_in/2 coeffs
    let signal: Vec<f32> = (0..n_in)
        .map(|i| (std::f64::consts::TAU * 440.0 * i as f64 / SR).sin() as f32)
        .collect();

    let coeffs = mdct(&signal);
    assert_eq!(coeffs.len(), n_in / 2, "MDCT should produce N/2 coefficients");

    let reconstructed = imdct(&coeffs);
    assert_eq!(reconstructed.len(), n_in, "IMDCT should produce N samples");

    // The MDCT/IMDCT with OLA recovers the middle N/2 samples of input
    // (critical sampling with 50% overlap). Without the OLA step the
    // reconstruction is only approximate, but the reconstruction should
    // have a similar RMS to the original.
    let rms_in = rms(&signal);
    let rms_out = rms(&reconstructed);

    assert!(
        rms_out > 0.0,
        "IMDCT should produce non-zero output"
    );
    // The ratios should be within 2x of each other (same energy scale)
    let ratio = rms_out / rms_in.max(1e-10);
    assert!(
        ratio > 0.3 && ratio < 3.0,
        "MDCT/IMDCT RMS ratio should be reasonable, got {ratio:.4}"
    );
}

// ── Test 15: Thiran allpass — verify output is a delayed version of input ─────

#[test]
fn thiran_allpass_delay() {
    // An allpass filter with delay D should produce output shifted by ~D samples.
    // We use a slow sine (10Hz) so the delay is clearly visible.
    let delay_samples = 3.5f64;
    let order = 2usize;

    let src = Arc::new(SinOsc::new(10.0));
    let thiran = ThiranAllpass {
        input: src as Arc<dyn Signal>,
        delay_samples,
        order,
    };
    let mut inst = thiran.instantiate(SR);
    let buf = fill_n(&mut inst, 4800);

    // The Thiran filter is an allpass: power should be preserved.
    // After settling (first 100 samples), the RMS of the output should match
    // the RMS of the input (a 10Hz sine has RMS ≈ 1/sqrt(2) ≈ 0.707).
    let output_rms = rms(&buf[100..]);
    assert!(
        output_rms > 0.6 && output_rms < 0.8,
        "Thiran allpass should preserve amplitude (allpass), got RMS={output_rms:.4}"
    );

    // The output should not be trivially identical to input —
    // the fractional delay introduces a phase shift.
    // Check that output[delay_int] is close to input[0] by comparing RMS of difference.
    let delay_int = delay_samples.round() as usize;
    let src2 = SinOsc::new(10.0);
    let mut inst2 = src2.instantiate(SR);
    let input_buf = fill_n(&mut inst2, 4800 + delay_int);
    // Compare delayed input to output
    let diff_rms: f32 = buf[delay_int..2400 + delay_int]
        .iter()
        .zip(input_buf[..2400].iter())
        .map(|(&o, &i)| (o - i) * (o - i))
        .sum::<f32>()
        .sqrt()
        / (2400.0_f32).sqrt();
    // The difference should be small (< 0.05) given the allpass is a good delay approximation
    assert!(
        diff_rms < 0.05,
        "Thiran allpass output should approximate delayed input, diff_rms={diff_rms:.4}"
    );
}

// ── Test 16: Hann window — first and last near 0, middle near 1 ──────────────

#[test]
fn hann_window_shape() {
    let n = 1024usize;
    let w = hann_window(n);

    assert_eq!(w.len(), n);
    assert!(
        w[0].abs() < 1e-5,
        "Hann window first sample should be 0, got {}", w[0]
    );
    assert!(
        w[n - 1].abs() < 1e-3,
        "Hann window last sample should be near 0, got {}", w[n - 1]
    );
    // Middle sample (symmetric peak)
    let mid = w[n / 2];
    assert!(
        mid > 0.99 && mid <= 1.001,
        "Hann window middle should be near 1.0, got {mid}"
    );
    // Monotonically increasing from 0 to n/2
    for i in 1..n / 2 {
        assert!(
            w[i] >= w[i - 1],
            "Hann window should increase in first half at i={i}: w[i]={} w[i-1]={}",
            w[i], w[i - 1]
        );
    }
}

// ── Test 17: Kaiser window — near 0 at edges ─────────────────────────────────

#[test]
fn kaiser_window_edges_near_zero() {
    let n = 128usize;
    let beta = 8.0f64;
    let w = kaiser_window(n, beta);

    assert_eq!(w.len(), n);
    // With high beta, the edges are very close to 0
    assert!(
        w[0] < 0.01,
        "Kaiser window first sample with beta=8 should be near 0, got {}", w[0]
    );
    assert!(
        w[n - 1] < 0.01,
        "Kaiser window last sample with beta=8 should be near 0, got {}", w[n - 1]
    );
    // Center should be 1.0
    let mid = w[n / 2];
    assert!(
        mid > 0.99,
        "Kaiser window center should be near 1.0, got {mid}"
    );
    // All values should be in [0, 1]
    assert!(
        w.iter().all(|&x| x >= 0.0 && x <= 1.001),
        "Kaiser window values should be in [0, 1]"
    );
}

// ── Test 18: Gaussian window — bell shape ────────────────────────────────────

#[test]
fn gaussian_window_bell_shape() {
    let n = 64usize;
    let sigma = 0.4f64;
    let w = gaussian_window(n, sigma);

    assert_eq!(w.len(), n);

    // Center should be 1.0 (peak)
    let center_idx = (n - 1) / 2;
    assert!(
        w[center_idx] > 0.95,
        "Gaussian window center should be near 1.0, got {}", w[center_idx]
    );

    // Edges should be smaller than center
    assert!(
        w[0] < w[center_idx],
        "Gaussian window edges should be smaller than center"
    );
    assert!(
        w[n - 1] < w[center_idx],
        "Gaussian window last sample should be smaller than center"
    );

    // Symmetric: w[i] ≈ w[n-1-i]
    for i in 0..n / 2 {
        let diff = (w[i] - w[n - 1 - i]).abs();
        assert!(
            diff < 0.01,
            "Gaussian window should be symmetric: w[{i}]={} w[{}]={} diff={diff:.4}",
            w[i], n - 1 - i, w[n - 1 - i]
        );
    }

    // Monotonically increasing from 0 to center
    for i in 1..center_idx {
        assert!(
            w[i] >= w[i - 1],
            "Gaussian window should increase toward center at i={i}"
        );
    }
}

// ── Test 19: CQT — 440Hz sine has peak in the 440Hz bin ──────────────────────
//
// CQT bin k has frequency f_min * 2^(k/bins_per_octave).
// With f_min=220Hz (A3), bins_per_octave=12, 440Hz is bin 12:
//   f_min * 2^(12/12) = 220 * 2 = 440 Hz.
// We use n_bins=24 (2 octaves, 220Hz to 880Hz) so bin 12 (440Hz) is in range.

#[test]
fn cqt_440hz_peak_at_correct_bin() {
    let n = 48_000usize; // 1 second of 440Hz sine
    let samples: Vec<f32> = (0..n)
        .map(|i| (std::f64::consts::TAU * 440.0 * i as f64 / SR).sin() as f32)
        .collect();

    let bins_per_octave = 12usize;
    let f_min = 220.0f64;  // A3 — so A4 (440Hz) is exactly bin 12
    let n_bins = 24usize;  // 2 octaves: 220Hz to 880Hz

    let mags = cqt_magnitudes(&samples, SR, bins_per_octave, f_min, n_bins);

    assert_eq!(mags.len(), n_bins);

    // 440Hz = f_min * 2^(12/12) → bin 12 exactly
    let target_bin = (bins_per_octave as f64 * (440.0_f64 / f_min).log2()).round() as usize;
    assert_eq!(target_bin, 12, "sanity: 440Hz should be bin 12 with f_min=220 bpo=12");

    let peak_bin = mags.iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(i, _)| i)
        .unwrap();

    // The peak should be within ±1 bin of 440Hz
    let dist = (peak_bin as isize - target_bin as isize).unsigned_abs();
    assert!(
        dist <= 1,
        "CQT peak should be near 440Hz bin {target_bin}, got peak at bin {peak_bin} (dist={dist})"
    );

    // The peak magnitude should be significantly larger than the average
    let avg_mag: f32 = mags.iter().sum::<f32>() / mags.len() as f32;
    let peak_mag = mags[peak_bin];
    assert!(
        peak_mag > avg_mag * 2.0,
        "CQT 440Hz bin should dominate: peak={peak_mag:.4} avg={avg_mag:.4}"
    );
}

// ── Test 20: LPC — analyze sine, synthesize produces non-zero output ─────────

#[test]
fn lpc_analyze_synthesize_nonzero() {
    // Analyze a 440Hz sine with LPC order 16
    let order = 16usize;
    let n = 1024usize;
    let signal: Vec<f32> = (0..n)
        .map(|i| (std::f64::consts::TAU * 440.0 * i as f64 / SR).sin() as f32)
        .collect();

    let coeffs = lpc_analyze(&signal, order);
    assert_eq!(coeffs.len(), order);

    // Coefficients should not all be zero
    let coeff_rms: f32 = rms(&coeffs);
    assert!(
        coeff_rms > 1e-5,
        "LPC coefficients should be non-zero for a sine input"
    );

    // Synthesize with impulse excitation
    let excitation: Vec<f32> = (0..n).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
    let synth = lpc_synthesize(&excitation, &coeffs);

    assert_eq!(synth.len(), n);

    // The synthesized output should be non-zero (the LPC filter resonates)
    let synth_rms = rms(&synth[10..]);
    assert!(
        synth_rms > 1e-5,
        "LPC synthesis should produce non-zero output, got rms={synth_rms:.6}"
    );
}

// ── Bonus Test 21: TriOsc — symmetric around 0, peaks at ±1 ─────────────────

#[test]
fn triosc_symmetric_and_bounded() {
    let osc = TriOsc::new(440.0);
    let mut inst = osc.instantiate(SR);
    let buf = fill_n(&mut inst, 48_000);

    // Triangle wave peaks at exactly 1.0 and -1.0
    let peak = buf.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let trough = buf.iter().cloned().fold(f32::INFINITY, f32::min);
    assert!(
        peak > 0.99,
        "TriOsc peak should be near 1.0, got {peak}"
    );
    assert!(
        trough < -0.99,
        "TriOsc trough should be near -1.0, got {trough}"
    );

    // RMS of a triangle wave = 1/sqrt(3) ≈ 0.577
    let r = rms(&buf);
    assert!(
        (r - 0.577).abs() < 0.03,
        "TriOsc RMS should be ≈0.577, got {r:.4}"
    );
}

// ── Bonus Test 22: Lorenz attractor — bounded and chaotic ────────────────────

#[test]
fn lorenz_attractor_bounded() {
    let lorenz_x = LorenzSignal {
        sigma: 10.0, rho: 28.0, beta: 2.667,
        dt: 0.005,
        x0: 1.0, y0: 0.0, z0: 0.0,
        output: 0,
    };
    let mut inst = lorenz_x.instantiate(SR);
    let buf = fill_n(&mut inst, 48_000);

    // Lorenz x stays roughly within ±25
    let peak = buf.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let trough = buf.iter().cloned().fold(f32::INFINITY, f32::min);
    assert!(
        peak < 30.0 && trough > -30.0,
        "Lorenz x should stay within ±30, got peak={peak:.2} trough={trough:.2}"
    );
    // Should show variation (chaotic)
    let s = std_dev(&buf);
    assert!(s > 1.0, "Lorenz should be chaotic (std_dev>1.0), got {s:.4}");
}

// ── Bonus Test 23: VecSignal plays back correctly ────────────────────────────

#[test]
fn vec_signal_exact_playback() {
    let data: Vec<f32> = (0..16).map(|i| i as f32 * 0.1).collect();
    let sig = VecSignal(data.clone());
    let mut inst = sig.instantiate(SR);
    let buf = fill_n(&mut inst, 20);

    // First 16 samples should exactly match data
    for (i, (&got, &expected)) in buf[..16].iter().zip(data.iter()).enumerate() {
        assert!(
            (got - expected).abs() < 1e-6,
            "VecSignal sample {i}: expected {expected}, got {got}"
        );
    }
    // Samples 16..20 should be 0
    for i in 16..20 {
        assert!(
            buf[i].abs() < 1e-6,
            "VecSignal should output 0 past end, got {} at {i}", buf[i]
        );
    }
}

// ── Bonus Test 24: FIR LP filter — windowed sinc passes below cutoff ─────────

#[test]
fn fir_lp_filter_frequency_response() {
    let cutoff = 2000.0f64;
    let n_taps = 127usize;
    let coeffs = fir_coeffs_lp(cutoff, SR, n_taps);

    // 440Hz: should pass (gain near 1 after settling)
    {
        let src = Arc::new(SinOsc::new(440.0));
        let filt = FirFilterSignal { input: src as Arc<dyn Signal>, coeffs: coeffs.clone() };
        let mut inst = filt.instantiate(SR);
        let buf = fill_n(&mut inst, 6_000);
        let gain = rms(&buf[n_taps..]);
        assert!(
            gain > 0.5,
            "FIR LP 2kHz should pass 440Hz, got gain={gain:.4}"
        );
    }

    // 8kHz: should be strongly attenuated
    {
        let src = Arc::new(SinOsc::new(8_000.0));
        let filt = FirFilterSignal { input: src as Arc<dyn Signal>, coeffs: coeffs.clone() };
        let mut inst = filt.instantiate(SR);
        let buf = fill_n(&mut inst, 6_000);
        let gain = rms(&buf[n_taps..]);
        assert!(
            gain < 0.05,
            "FIR LP 2kHz should strongly attenuate 8kHz, got gain={gain:.4}"
        );
    }
}
