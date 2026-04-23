//! End-to-end smoke test: play a 440 Hz sine for 1 second.
//! Run with: `cargo run --bin stax-smoke`
//!
//! This is deliberately bypasses the interpreter. Once M1 is in, we'll
//! have a parallel example that reaches the same audio through
//! `440 0 sinosc .3 * play`.

use std::sync::Arc;

use stax_dsp::SinOsc;

fn main() -> anyhow::Result<()> {
    println!("stax-smoke: playing 440 Hz sine for 1 second");
    let osc = Arc::new(SinOsc::new(440.0));
    stax_audio::play_blocking(osc, 1.0)?;
    println!("done");
    Ok(())
}
