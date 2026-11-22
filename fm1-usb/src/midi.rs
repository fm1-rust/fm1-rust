//! Class-driver MIDI connections and discovery of the FM-1 log responder.

use super::{log, only};
use core::{fmt, time::Duration};
use midir::{
    MidiIO, MidiInput, MidiInputConnection, MidiInputPort, MidiOutput, MidiOutputConnection,
    MidiOutputPort,
};
use std::{
    io::{self, ErrorKind},
    sync::mpsc::{self, Receiver},
};

const CLIENT: &str = "fm1-usb";

/// Native MIDI ports for both directions of one connection.
#[derive(Clone)]
pub struct MidiPorts {
    input: MidiInputPort,
    output: MidiOutputPort,
}

impl fmt::Debug for MidiPorts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MidiPorts")
            .field("input", &self.input.id())
            .field("output", &self.output.id())
            .finish()
    }
}

impl MidiPorts {
    /// Select a MIDI input and output by case-insensitive name substring.
    /// This also works with firmware that only supports musical MIDI messages.
    ///
    /// # Errors
    /// Returns an error unless exactly one port matches in each direction.
    pub fn by_name(name: &str) -> io::Result<Self> {
        fn named<M: MidiIO>(midi: M, name: &str, direction: &str) -> io::Result<M::Port> {
            let needle = name.to_lowercase();
            let ports = midi.ports().into_iter().filter(|port| {
                midi.port_name(port)
                    .is_ok_and(|s| s.to_lowercase().contains(&needle))
            });
            only(ports, &format!("MIDI {direction} matching {name:?}"))
        }
        let input = named(
            MidiInput::new(CLIENT).map_err(io::Error::other)?,
            name,
            "input",
        )?;
        let output = named(
            MidiOutput::new(CLIENT).map_err(io::Error::other)?,
            name,
            "output",
        )?;
        Ok(Self { input, output })
    }

    /// Open both directions. The callback receives each complete MIDI message
    /// with the input port's microsecond timestamp, including SysEx replies.
    ///
    /// # Errors
    /// Returns an error if either port is unavailable.
    pub fn connect(
        &self,
        mut on_midi: impl FnMut(u64, &[u8]) + Send + 'static,
    ) -> io::Result<MidiConnection> {
        let (sender, replies) = mpsc::sync_channel(1);
        let input = listen(&self.input, move |time, message| {
            if let Some(text) = log::snapshot(message) {
                let _ = sender.try_send(text.to_owned());
            }
            on_midi(time, message);
        })?;
        Ok(MidiConnection {
            _input: input,
            output: output(&self.output)?,
            replies,
        })
    }
}

/// A live bidirectional MIDI connection. Dropping it closes both ports.
pub struct MidiConnection {
    _input: MidiInputConnection<()>,
    output: MidiOutputConnection,
    replies: Receiver<String>,
}

impl fmt::Debug for MidiConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MidiConnection").finish_non_exhaustive()
    }
}

impl MidiConnection {
    /// Send one complete MIDI message, including the status byte.
    ///
    /// # Errors
    /// Returns an error if the MIDI backend cannot send the message.
    pub fn send(&mut self, message: &[u8]) -> io::Result<()> {
        self.output.send(message).map_err(io::Error::other)
    }

    /// Request one ASCII log snapshot, preserving all bytes and line endings.
    ///
    /// # Errors
    /// Returns an error on send failure, disconnect or timeout.
    pub fn read_log(&mut self, timeout: Duration) -> io::Result<String> {
        log::request(&self.replies, timeout, |request| {
            self.output.send(request).map_err(io::Error::other)
        })
    }
}

fn listen(
    port: &MidiInputPort,
    mut callback: impl FnMut(u64, &[u8]) + Send + 'static,
) -> io::Result<MidiInputConnection<()>> {
    MidiInput::new(CLIENT)
        .map_err(io::Error::other)?
        .connect(port, CLIENT, move |t, msg, ()| callback(t, msg), ())
        .map_err(|e| io::Error::other(e.to_string()))
}

fn output(port: &MidiOutputPort) -> io::Result<MidiOutputConnection> {
    MidiOutput::new(CLIENT)
        .map_err(io::Error::other)?
        .connect(port, CLIENT)
        .map_err(|e| io::Error::other(e.to_string()))
}

pub(super) fn probe(product: Option<&str>, timeout: Duration) -> io::Result<(MidiPorts, String)> {
    let inputs = MidiInput::new(CLIENT).map_err(io::Error::other)?.ports();
    let midi = MidiOutput::new(CLIENT).map_err(io::Error::other)?;
    let mut outputs = midi.ports();
    // Try the USB descriptor's product name first if the OS happens to use it.
    // Every output remains eligible when CoreMIDI caches an older name.
    if let Some(product) = product {
        let product = product.to_lowercase();
        outputs.sort_by_cached_key(|p| {
            !midi
                .port_name(p)
                .is_ok_and(|n| n.to_lowercase().contains(&product))
        });
    }

    probe_ports(&inputs, outputs, timeout)
}

fn probe_ports(
    inputs: &[MidiInputPort],
    outputs: Vec<MidiOutputPort>,
    timeout: Duration,
) -> io::Result<(MidiPorts, String)> {
    // Subscribe before sending. A studio with many inputs needs just one
    // request per unrelated output, regardless of the input port count.
    let (sender, replies) = mpsc::sync_channel(1);
    let mut listeners = Vec::new();
    let mut error = io::Error::new(ErrorKind::NotFound, "no MIDI inputs found");
    for (index, port) in inputs.iter().enumerate() {
        let sender = sender.clone();
        match listen(port, move |_, message| {
            if let Some(text) = log::snapshot(message) {
                let _ = sender.try_send((index, text.to_owned()));
            }
        }) {
            Ok(listener) => listeners.push(listener),
            Err(e) => error = e,
        }
    }
    drop(sender);
    if listeners.is_empty() {
        return Err(error);
    }

    let mut error = io::Error::new(ErrorKind::NotFound, "no MIDI outputs found");
    for port in outputs {
        match probe_output(&port, &replies, timeout) {
            Ok((input, text)) => {
                let ports = MidiPorts {
                    input: inputs[input].clone(),
                    output: port,
                };
                return Ok((ports, text));
            }
            Err(e) => error = e,
        }
    }
    Err(io::Error::new(
        error.kind(),
        format!("no MIDI pair answered the FM-1 log request ({error})"),
    ))
}

fn probe_output(
    port: &MidiOutputPort,
    replies: &Receiver<(usize, String)>,
    timeout: Duration,
) -> io::Result<(usize, String)> {
    let mut tx = output(port)?;
    let mut read = || {
        log::request(replies, timeout, |msg| {
            tx.send(msg).map_err(io::Error::other)
        })
    };
    let (first, _) = read()?;
    // A late reply from a previous output might arrive during the first
    // request. Confirm that a second request reaches the same input.
    let (second, text) = read()?;
    if first != second {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "log replies arrived on different MIDI inputs",
        ));
    }
    Ok((second, text))
}

#[cfg(all(test, unix))]
mod tests;
