//! Isolate socket backpressure from the shared daemon event loop.
use std::{
    net::Shutdown,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use super::{BridgeEnvelope, CLIENT_WRITE_TIMEOUT, LocalStream, write_client_frame};
use anyhow::{Result, bail};

const MAX_PENDING_FRAMES: usize = 64;
// This writer runs on its own thread, so a long stall allowance never delays
// the daemon loop. Clients that pause briefly (rendering a long transcript,
// a GC pause in the gateway) stay attached; truly stuck ones are still
// dropped by this deadline or by the bounded queue.
const CLIENT_STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;

struct PendingFrame {
    bytes: Arc<[u8]>,
    pending_bytes: Arc<AtomicUsize>,
}

impl Drop for PendingFrame {
    fn drop(&mut self) {
        self.pending_bytes
            .fetch_sub(self.bytes.len(), Ordering::Relaxed);
    }
}

pub(super) struct ClientWriter {
    stream: LocalStream,
    tx: mpsc::SyncSender<PendingFrame>,
    pending_bytes: Arc<AtomicUsize>,
}

impl ClientWriter {
    pub(super) fn new(stream: LocalStream) -> Result<Self> {
        let mut output = stream.try_clone()?;
        // Darwin can reject SO_SNDTIMEO on AF_UNIX with EINVAL. The daemon's
        // accept path already treats that as an unsupported socket option; the
        // writer clone must do the same or a perfectly healthy client can fail
        // attachment after the transport handshake. Backpressure is still
        // bounded by the dedicated writer thread, queue/byte budgets and
        // disconnect-on-overflow semantics below.
        if let Err(error) = output.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
            && error.kind() != std::io::ErrorKind::InvalidInput
        {
            return Err(error.into());
        }
        let (tx, rx) = mpsc::sync_channel::<PendingFrame>(MAX_PENDING_FRAMES);
        thread::Builder::new()
            .name("yeet-client-writer".into())
            .spawn(move || {
                for frame in rx {
                    if write_client_frame(&mut output, frame.bytes.as_ref(), CLIENT_STALL_TIMEOUT)
                        .is_err()
                    {
                        break;
                    }
                }
                // Wake the reader on write failure so the daemon removes this client.
                let _ = output.shutdown(Shutdown::Both);
            })?;
        Ok(Self {
            stream,
            tx,
            pending_bytes: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub(super) fn encode(envelope: &BridgeEnvelope) -> Result<Arc<[u8]>> {
        let mut bytes = serde_json::to_vec(envelope)?;
        bytes.push(b'\n');
        Ok(bytes.into())
    }

    pub(super) fn send(&self, envelope: &BridgeEnvelope) -> Result<()> {
        self.send_frame(Self::encode(envelope)?)
    }

    pub(super) fn send_frame(&self, bytes: Arc<[u8]>) -> Result<()> {
        if self
            .pending_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |pending| {
                pending
                    .checked_add(bytes.len())
                    .filter(|total| *total <= MAX_PENDING_BYTES)
            })
            .is_err()
        {
            self.disconnect();
            bail!("background client outbound byte limit exceeded");
        }
        let frame = PendingFrame {
            bytes,
            pending_bytes: Arc::clone(&self.pending_bytes),
        };
        if self.tx.try_send(frame).is_err() {
            // Never drop a protocol frame and continue: reconnect for a fresh state.
            self.disconnect();
            bail!("background client outbound queue is full or closed");
        }
        Ok(())
    }

    pub(super) fn disconnect(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

impl Drop for ClientWriter {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};

    fn envelope(message: &str) -> BridgeEnvelope {
        BridgeEnvelope {
            kind: "error".into(),
            state: None,
            message: Some(message.into()),
        }
    }

    #[test]
    fn delivers_frames_in_order_and_closes_on_drop() {
        let (socket, peer) = LocalStream::pair().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let writer = ClientWriter::new(socket).unwrap();
        writer.send(&envelope("first")).unwrap();
        writer.send(&envelope("second")).unwrap();
        let mut reader = BufReader::new(peer);
        for expected in ["first", "second"] {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let received: BridgeEnvelope = serde_json::from_str(&line).unwrap();
            assert_eq!(received.message.as_deref(), Some(expected));
        }
        drop(writer);
        assert_eq!(reader.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn full_queue_disconnects_only_slow_client_and_releases_budget() {
        let (socket, mut peer) = LocalStream::pair().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        // A receiver deliberately not consuming simulates a stalled socket writer.
        let (tx, rx) = mpsc::sync_channel(1);
        let writer = ClientWriter {
            stream: socket,
            tx,
            pending_bytes: Arc::new(AtomicUsize::new(0)),
        };
        writer.send(&envelope("queued")).unwrap();
        assert!(writer.send(&envelope("overflow")).is_err());
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        drop(rx);
        assert_eq!(writer.pending_bytes.load(Ordering::Relaxed), 0);
        let (healthy, peer) = LocalStream::pair().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let healthy = ClientWriter::new(healthy).unwrap();
        healthy.send(&envelope("still responsive")).unwrap();
        let mut line = String::new();
        BufReader::new(peer).read_line(&mut line).unwrap();
        assert!(line.contains("still responsive"));
    }

    #[test]
    fn byte_budget_disconnects_before_enqueueing() {
        let (socket, _peer) = LocalStream::pair().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let writer = ClientWriter {
            stream: socket,
            tx,
            pending_bytes: Arc::new(AtomicUsize::new(MAX_PENDING_BYTES)),
        };
        assert!(writer.send(&envelope("over budget")).is_err());
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}
