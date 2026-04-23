//! Audio I/O. `cpal` on native, stub on WASM (AudioWorklet bridge lands in M6).
//!
//! The contract is small: give us a `Box<dyn SignalInstance>` already
//! instantiated at a known sample rate, and we'll pump it to the speakers
//! until `stop()` is called.

use stax_core::SignalInstance;

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(target_arch = "wasm32")]
pub use web::*;

// -------------------------------------------------------------------------
// Native (cpal)
// -------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{Arc, Mutex};

    use anyhow::{anyhow, Result};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    use stax_core::Signal;

    /// Handle to a running voice. Drop it to stop the voice.
    pub struct Voice {
        _stream: cpal::Stream,
    }

    pub struct Runtime {
        host: cpal::Host,
        device: cpal::Device,
        config: cpal::StreamConfig,
        sample_rate: f64,
    }

    impl Runtime {
        pub fn new() -> Result<Self> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or_else(|| anyhow!("no default output device"))?;
            let supported = device.default_output_config()?;
            let sample_rate = supported.sample_rate().0 as f64;
            let config: cpal::StreamConfig = supported.into();
            Ok(Self { host, device, config, sample_rate })
        }

        pub fn sample_rate(&self) -> f64 {
            self.sample_rate
        }

        pub fn host_name(&self) -> String {
            self.host.id().name().to_string()
        }

        /// Spin up a voice that plays `signal` until the returned `Voice` is dropped.
        pub fn play(&self, signal: Arc<dyn Signal>) -> Result<Voice> {
            let channels = self.config.channels as usize;
            let instance = Arc::new(Mutex::new(signal.instantiate(self.sample_rate)));
            // Temporary scratch matches one channel at a time; for multi-channel
            // the signal is expected to interleave itself, matching SAPF's
            // multi-channel expansion semantics.
            let err_fn = |e| eprintln!("audio stream error: {e}");

            // Pre-allocated mono scratch avoids per-callback heap allocation.
            let mut mono_scratch = Vec::with_capacity(4096);
            let stream = self.device.build_output_stream(
                &self.config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let mut inst = instance.lock().unwrap();
                    if inst.channels() == channels {
                        inst.fill(out);
                    } else if inst.channels() == 1 && channels > 1 {
                        let frames = out.len() / channels;
                        mono_scratch.resize(frames, 0.0f32);
                        inst.fill(&mut mono_scratch[..frames]);
                        for i in 0..frames {
                            let s = mono_scratch[i];
                            for c in 0..channels {
                                out[i * channels + c] = s;
                            }
                        }
                    } else {
                        out.fill(0.0);
                    }
                },
                err_fn,
                None,
            )?;
            stream.play()?;
            Ok(Voice { _stream: stream })
        }
    }
}

// -------------------------------------------------------------------------
// WASM stub — real implementation lands in M6 via AudioWorklet
// -------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::Arc;

    use stax_core::Signal;

    pub struct Voice;

    pub struct Runtime;

    impl Runtime {
        pub fn new() -> anyhow::Result<Self> { Ok(Self) }
        pub fn sample_rate(&self) -> f64 { 48_000.0 }
        pub fn host_name(&self) -> String { "web-audio (M6)".into() }
        pub fn play(&self, _signal: Arc<dyn Signal>) -> anyhow::Result<Voice> {
            anyhow::bail!("WASM audio runtime is an M6 deliverable");
        }
    }
}

// -------------------------------------------------------------------------

// A tiny wrapper that some callers may want: "play this for N seconds
// synchronously." Useful for the CLI smoke test and for headless tests.
#[cfg(not(target_arch = "wasm32"))]
pub fn play_blocking(
    signal: std::sync::Arc<dyn stax_core::Signal>,
    seconds: f32,
) -> anyhow::Result<()> {
    let rt = native::Runtime::new()?;
    let _voice = rt.play(signal)?;
    std::thread::sleep(std::time::Duration::from_secs_f32(seconds));
    Ok(())
}

// Keep the trait object in scope for doc-linking convenience.
#[allow(dead_code)]
fn _use_instance(_i: Box<dyn SignalInstance>) {}
