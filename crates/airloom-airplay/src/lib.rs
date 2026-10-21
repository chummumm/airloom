//! Original AirPlay 2 sender. A connected session is not proof of acoustic output.
//! All network inputs are bounded; authentication failures fail closed.

mod audio;
mod crypto;
mod discovery;
mod pairing;
mod rtsp;
mod sender;
mod timing;
mod tlv;

pub use discovery::{discover, Device};
pub use pairing::{pair, PairingCredentials, PairingSession};
pub use sender::{ConnectOptions, Sender, SenderMetrics, TimingMode, VolumeControl};

use serde::{Deserialize, Serialize};

pub const SAMPLE_RATE: u32 = 44_100;
pub const CHANNELS: usize = 2;
pub const FRAMES_PER_PACKET: usize = 352;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationKind {
    Password,
    Pin,
    Credentials,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("authentication required ({kind:?})")]
    AuthenticationRequired { kind: AuthenticationKind },
    #[error("authentication rejected ({kind:?})")]
    AuthenticationRejected { kind: AuthenticationKind },
    #[error("receiver denied access at {stage} (status {status})")]
    AccessDenied { stage: &'static str, status: u16 },
    #[error("receiver rejected {stage} (status {status})")]
    Receiver { stage: &'static str, status: u16 },
    #[error("unsupported receiver capability: {0}")]
    Unsupported(String),
    #[error("invalid protocol data: {0}")]
    Protocol(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("operation timed out: {0}")]
    Timeout(&'static str),
    #[error("session has stopped")]
    Stopped,
    #[error("session lost: {0}")]
    SessionLost(String),
    #[error("device discovery: {0}")]
    Discovery(String),
    #[error("network I/O: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

fn protocol(message: impl Into<String>) -> Error {
    Error::Protocol(message.into())
}
