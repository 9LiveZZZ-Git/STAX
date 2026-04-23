//! MIDI in/out via `midir`, OSC via `rosc`.
//!
//! Designed for M3: lets stax talk to hardware synths, DAWs, TidalCycles, etc.
//! All I/O is synchronous from the stax-eval perspective — MIDI messages are
//! sent immediately; OSC is fire-and-forget UDP.

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(target_arch = "wasm32")]
pub use stub::*;

// ---- native -----------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use anyhow::{anyhow, Result};
    use midir::{MidiOutput, MidiOutputConnection};

    /// A live MIDI output connection.
    /// `Send` wrapper — midir connections are internally thread-safe on all
    /// supported platforms despite not implementing `Send` automatically.
    pub struct MidiOut {
        conn: MidiOutputConnection,
    }

    // Safety: midir wraps OS MIDI handles; single-threaded use is always safe.
    // The stax interpreter is single-threaded.
    unsafe impl Send for MidiOut {}

    impl MidiOut {
        /// List available MIDI output port names.
        pub fn ports() -> Vec<String> {
            MidiOutput::new("stax")
                .ok()
                .map(|mo| {
                    mo.ports()
                        .iter()
                        .filter_map(|p| mo.port_name(p).ok())
                        .collect()
                })
                .unwrap_or_default()
        }

        /// Open the MIDI output port at `port_idx` (0-based index into `ports()`).
        pub fn connect(port_idx: usize) -> Result<Self> {
            let mo = MidiOutput::new("stax")?;
            let ports = mo.ports();
            let port = ports
                .get(port_idx)
                .ok_or_else(|| anyhow!("MIDI output port {port_idx} not found"))?;
            let conn = mo.connect(port, "stax-out")?;
            Ok(Self { conn })
        }

        /// Send raw MIDI bytes.
        pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
            self.conn.send(bytes)?;
            Ok(())
        }

        /// Convenience: send a Note On message.
        pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) -> Result<()> {
            self.send(&[0x90 | (channel & 0x0f), note & 0x7f, velocity & 0x7f])
        }

        /// Convenience: send a Note Off message.
        pub fn note_off(&mut self, channel: u8, note: u8, velocity: u8) -> Result<()> {
            self.send(&[0x80 | (channel & 0x0f), note & 0x7f, velocity & 0x7f])
        }

        /// Convenience: send a Control Change message.
        pub fn cc(&mut self, channel: u8, controller: u8, value: u8) -> Result<()> {
            self.send(&[0xb0 | (channel & 0x0f), controller & 0x7f, value & 0x7f])
        }
    }

    // ---- MIDI In ------------------------------------------------------------

    use midir::{MidiInput, MidiInputConnection};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A live MIDI input connection with an internal message queue.
    ///
    /// Messages arrive in a background callback thread and are queued for
    /// `read()` to drain from the interpreter thread.
    pub struct MidiIn {
        /// Keep the connection alive; dropped when struct is dropped.
        _conn: MidiInputConnection<()>,
        queue: Arc<Mutex<VecDeque<[u8; 3]>>>,
    }

    unsafe impl Send for MidiIn {}

    impl MidiIn {
        pub fn ports() -> Vec<String> {
            MidiInput::new("stax-in")
                .ok()
                .map(|mi| {
                    mi.ports()
                        .iter()
                        .filter_map(|p| mi.port_name(p).ok())
                        .collect()
                })
                .unwrap_or_default()
        }

        pub fn connect(port_idx: usize) -> Result<Self> {
            let mi = MidiInput::new("stax-in")?;
            let ports = mi.ports();
            let port = ports
                .get(port_idx)
                .ok_or_else(|| anyhow!("MIDI input port {port_idx} not found"))?;
            let queue: Arc<Mutex<VecDeque<[u8; 3]>>> = Arc::new(Mutex::new(VecDeque::new()));
            let q2 = Arc::clone(&queue);
            let conn = mi.connect(port, "stax-in", move |_stamp, msg, _| {
                if msg.len() >= 3 {
                    if let Ok(mut q) = q2.lock() {
                        q.push_back([msg[0], msg[1], msg[2]]);
                    }
                }
            }, ())?;
            Ok(Self { _conn: conn, queue })
        }

        /// Drain one MIDI message `[status, data1, data2]` from the queue, if any.
        pub fn read(&self) -> Option<[u8; 3]> {
            self.queue.lock().ok()?.pop_front()
        }
    }

    // ---- OSC ----------------------------------------------------------------

    pub use rosc::OscType;
    use rosc::{encoder, OscMessage, OscPacket};
    use std::net::UdpSocket;

    /// Send a single OSC message to `host:port`.
    pub fn osc_send(host: &str, port: u16, addr: &str, args: Vec<OscType>) -> Result<()> {
        let msg = OscPacket::Message(OscMessage { addr: addr.to_string(), args });
        let bytes = encoder::encode(&msg)?;
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.send_to(&bytes, format!("{host}:{port}"))?;
        Ok(())
    }
}

// ---- WASM stub --------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod stub {
    use anyhow::bail;

    pub struct MidiOut;

    impl MidiOut {
        pub fn ports() -> Vec<String> { vec![] }
        pub fn connect(_port_idx: usize) -> anyhow::Result<Self> {
            bail!("MIDI is not available in WASM (M6)")
        }
        pub fn send(&mut self, _bytes: &[u8]) -> anyhow::Result<()> {
            bail!("MIDI is not available in WASM")
        }
        pub fn note_on(&mut self, _ch: u8, _n: u8, _v: u8) -> anyhow::Result<()> {
            bail!("MIDI is not available in WASM")
        }
        pub fn note_off(&mut self, _ch: u8, _n: u8, _v: u8) -> anyhow::Result<()> {
            bail!("MIDI is not available in WASM")
        }
        pub fn cc(&mut self, _ch: u8, _ctrl: u8, _v: u8) -> anyhow::Result<()> {
            bail!("MIDI is not available in WASM")
        }
    }

    pub struct MidiIn;

    impl MidiIn {
        pub fn ports() -> Vec<String> { vec![] }
        pub fn connect(_port_idx: usize) -> anyhow::Result<Self> {
            bail!("MIDI in is not available in WASM (M6)")
        }
        pub fn read(&self) -> Option<[u8; 3]> { None }
    }

    pub enum OscType { Int(i32) }

    pub fn osc_send(_host: &str, _port: u16, _addr: &str, _args: Vec<OscType>) -> anyhow::Result<()> {
        bail!("OSC is not available in WASM")
    }
}
