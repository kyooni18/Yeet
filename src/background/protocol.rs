//! Compatibility contract between a frontend client and the workspace
//! background daemon.
//!
//! The daemon's first frame on every accepted socket is a `heartbeat`
//! envelope. Since protocol version 1 that frame also carries a `handshake`
//! object describing what the daemon speaks. Older clients ignore the extra
//! field (they only look at `type`), and newer clients treat a heartbeat
//! without a handshake as a legacy daemon of the original contract, so a
//! daemon started by a previous Yeet binary keeps its sessions alive across
//! a local install.
//!
//! Versions are deliberately separate:
//! - `protocol_version`: framing and the command/envelope transport itself.
//!   A mismatch means the peers cannot exchange frames safely.
//! - `state_schema_version`: the semantic `HarnessState` (`BridgeState`)
//!   carried in envelopes. A client accepts any schema in
//!   `MIN_STATE_SCHEMA_VERSION..=STATE_SCHEMA_VERSION`.
//! - `features`: additive capabilities a client may gate optional behavior
//!   on without bumping either version.
//!
//! An incompatible daemon is never retired by the client: the caller reports
//! it and leaves existing sessions running (see `BackgroundConnection`).

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::model::BridgeEnvelope;

/// Background transport version spoken by this binary.
pub(crate) const PROTOCOL_VERSION: u32 = 1;
/// Semantic state schema emitted by this binary's runtimes.
pub(crate) const STATE_SCHEMA_VERSION: u32 = 1;
/// Oldest daemon state schema this client can render.
pub(crate) const MIN_STATE_SCHEMA_VERSION: u32 = 1;

/// Additive daemon capabilities. Names are stable wire identifiers.
pub(crate) mod features {
    /// Each session runs in its own isolated runtime inside the daemon.
    pub(crate) const ISOLATED_SESSION_RUNTIMES: &str = "isolated-session-runtimes";
    /// A process-wide owner lease prevents two live daemons per workspace.
    pub(crate) const OWNER_LEASE: &str = "owner-lease";
    /// `LoadSession` / `NewSession` route to a per-session runtime.
    pub(crate) const SESSION_ROUTING: &str = "session-routing";
}

/// Daemon self-description sent in the first frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonHandshake {
    pub(crate) protocol_version: u32,
    pub(crate) state_schema_version: u32,
    #[serde(default)]
    pub(crate) features: Vec<String>,
    /// Crate version of the daemon binary, for diagnostics only.
    #[serde(default)]
    pub(crate) daemon_version: String,
    #[serde(default)]
    pub(crate) pid: u32,
}

impl DaemonHandshake {
    /// The handshake this binary's daemon advertises.
    pub(crate) fn current() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            state_schema_version: STATE_SCHEMA_VERSION,
            features: [
                features::ISOLATED_SESSION_RUNTIMES,
                features::OWNER_LEASE,
                features::SESSION_ROUTING,
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
        }
    }

    /// The contract assumed for a daemon that predates the handshake field.
    pub(crate) fn legacy() -> Self {
        Self {
            protocol_version: 1,
            state_schema_version: 1,
            features: Vec::new(),
            daemon_version: String::new(),
            pid: 0,
        }
    }

    /// Fails when this client cannot safely talk to the described daemon.
    pub(crate) fn ensure_compatible(&self) -> Result<()> {
        if self.protocol_version != PROTOCOL_VERSION {
            bail!(
                "background protocol version {} is not supported by this client (expects {PROTOCOL_VERSION}); daemon version {}",
                self.protocol_version,
                self.display_version()
            );
        }
        if !(MIN_STATE_SCHEMA_VERSION..=STATE_SCHEMA_VERSION).contains(&self.state_schema_version) {
            bail!(
                "background state schema {} is outside this client's supported range {MIN_STATE_SCHEMA_VERSION}..={STATE_SCHEMA_VERSION}; daemon version {}",
                self.state_schema_version,
                self.display_version()
            );
        }
        Ok(())
    }

    fn display_version(&self) -> &str {
        if self.daemon_version.is_empty() {
            "unknown"
        } else {
            &self.daemon_version
        }
    }
}

/// Wire shape of the daemon's first frame: a heartbeat envelope plus the
/// handshake. Kept separate from `BridgeEnvelope` so ordinary envelopes are
/// unchanged on the wire.
#[derive(Serialize)]
struct ReadyFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    state: Option<()>,
    message: Option<()>,
    handshake: &'a DaemonHandshake,
}

/// Serializes the daemon's ready frame (without the trailing newline).
pub(crate) fn ready_frame(handshake: &DaemonHandshake) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&ReadyFrame {
        kind: "heartbeat",
        state: None,
        message: None,
        handshake,
    })?)
}

#[derive(Deserialize)]
struct HandshakeProbe {
    #[serde(default)]
    handshake: Option<DaemonHandshake>,
}

/// Parses the daemon's first frame into its envelope and negotiated contract.
pub(crate) fn parse_first_frame(frame: &[u8]) -> Result<(BridgeEnvelope, DaemonHandshake)> {
    let envelope = serde_json::from_slice::<BridgeEnvelope>(frame)?;
    let handshake = if envelope.kind == "heartbeat" {
        serde_json::from_slice::<HandshakeProbe>(frame)?
            .handshake
            .unwrap_or_else(DaemonHandshake::legacy)
    } else {
        DaemonHandshake::legacy()
    };
    Ok((envelope, handshake))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_frame_round_trips_and_stays_a_heartbeat_for_old_clients() {
        let frame = ready_frame(&DaemonHandshake::current()).unwrap();
        let envelope = serde_json::from_slice::<BridgeEnvelope>(&frame).unwrap();
        assert_eq!(envelope.kind, "heartbeat");
        assert!(envelope.state.is_none());

        let (_, handshake) = parse_first_frame(&frame).unwrap();
        assert_eq!(handshake, DaemonHandshake::current());
        assert!(
            handshake
                .features
                .iter()
                .any(|f| f == features::OWNER_LEASE)
        );
        handshake.ensure_compatible().unwrap();
    }

    #[test]
    fn pre_handshake_daemon_is_accepted_as_legacy() {
        let frame = br#"{"type":"heartbeat","state":null,"message":null}"#;
        let (_, handshake) = parse_first_frame(frame).unwrap();
        assert_eq!(handshake, DaemonHandshake::legacy());
        handshake.ensure_compatible().unwrap();
    }

    #[test]
    fn mismatched_protocol_or_newer_schema_is_rejected() {
        let mut other = DaemonHandshake::current();
        other.protocol_version = PROTOCOL_VERSION + 1;
        assert!(other.ensure_compatible().is_err());

        let mut newer = DaemonHandshake::current();
        newer.state_schema_version = STATE_SCHEMA_VERSION + 1;
        assert!(newer.ensure_compatible().is_err());
    }
}
