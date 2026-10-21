//! Driverless, shared-mode Windows playback capture.
//!
//! Every chunk contains finite interleaved stereo `f32` samples at the requested
//! sample rate. WASAPI, native formats and COM apartment ownership stay inside
//! this crate. No audio is captured on unsupported platforms.

mod application;
pub mod credentials;
mod source_changes;
pub use source_changes::{watch_sources, SourceChanges, SourceSnapshot};

#[cfg(any(windows, test))]
mod mute_policy;
#[cfg(any(windows, test))]
mod pipeline;
#[cfg(windows)]
#[path = "platform/windows.rs"]
mod platform;

use std::time::Duration;
use thiserror::Error;

pub const DEFAULT_SOURCE_ID: &str = "default";
pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;
pub const OUTPUT_CHANNELS: usize = 2;

pub type Result<T> = std::result::Result<T, CaptureError>;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("Audio capture and credential storage require Windows")]
    UnsupportedPlatform,
    #[error("Invalid capture setting: {0}")]
    InvalidConfig(&'static str),
    #[error("Unsupported playback format: {0}")]
    UnsupportedFormat(String),
    #[error("Windows operation {operation} failed (HRESULT {code:#010x})")]
    Windows { operation: &'static str, code: i32 },
    #[error("The capture worker has stopped")]
    WorkerStopped,
    #[error("The capture worker panicked")]
    WorkerPanicked,
    #[error("Could not start the capture worker: {0}")]
    WorkerStart(std::io::Error),
    #[error("Timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("Audio conversion failed: {0}")]
    Conversion(String),
    #[error("Credential data exceeds Windows Credential Manager's size limit")]
    CredentialTooLarge,
    #[error(
        "The selected application has exited; refresh the application list and select it again"
    )]
    ApplicationExited,
    #[error("Local endpoint mute is not available when capturing one application")]
    ApplicationMuteUnsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSource {
    /// A Windows endpoint ID, or an application ID bound to one process lifetime.
    /// The special endpoint ID `default` is also accepted.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureInfo {
    /// Resolved endpoint ID or lifetime-bound application ID.
    pub source_id: String,
    pub source_name: String,
}

#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub source_id: String,
    pub target_sample_rate: u32,
    /// Number of audio chunks retained. Oldest chunks are discarded on overload.
    pub queue_capacity: usize,
    /// Opt-in only. This changes the chosen local playback endpoint's mute state.
    /// Whether loopback is pre-mute is device/driver dependent; it is not promised.
    pub mute_local: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            source_id: DEFAULT_SOURCE_ID.into(),
            target_sample_rate: DEFAULT_SAMPLE_RATE,
            queue_capacity: 8,
            mute_local: false,
        }
    }
}

impl CaptureConfig {
    fn validate(&self) -> Result<()> {
        if self.source_id.is_empty() || self.source_id.contains('\0') {
            return Err(CaptureError::InvalidConfig(
                "source ID is empty or contains NUL",
            ));
        }
        if is_application_source(&self.source_id) {
            application::ApplicationId::parse(&self.source_id)?;
            if self.mute_local {
                return Err(CaptureError::ApplicationMuteUnsupported);
            }
        }
        if !(8_000..=192_000).contains(&self.target_sample_rate) {
            return Err(CaptureError::InvalidConfig(
                "sample rate must be 8000–192000 Hz",
            ));
        }
        if !(1..=64).contains(&self.queue_capacity) {
            return Err(CaptureError::InvalidConfig(
                "queue capacity must be 1–64 chunks",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AudioChunk {
    /// Left, right, left, right; finite values clamped to [-1, 1].
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub sequence: u64,
    pub rms: [f32; 2],
    pub peak: [f32; 2],
    /// Cumulative output frames discarded because the bounded queue was full.
    pub dropped_frames: u64,
}

impl AudioChunk {
    pub fn frames(&self) -> usize {
        self.samples.len() / OUTPUT_CHANNELS
    }
}

/// Enumerates active playback endpoints on a separate COM-initialized thread.
/// Default selection is represented by `is_default`; `default` remains a valid
/// start ID even when the caller does not enumerate first.
pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    #[cfg(windows)]
    {
        platform::enumerate_sources()
    }
    #[cfg(not(windows))]
    {
        Err(CaptureError::UnsupportedPlatform)
    }
}

/// Lists applications with a nonexpired Windows audio session, including paused
/// sessions. Browser and other
/// same-executable subprocesses are grouped under their live app root. IDs are
/// scoped to the current process lifetime and must not be reused after restart.
pub fn enumerate_applications() -> Result<Vec<CaptureSource>> {
    #[cfg(windows)]
    {
        platform::enumerate_applications()
    }
    #[cfg(not(windows))]
    {
        Err(CaptureError::UnsupportedPlatform)
    }
}

/// Identifies the application ID namespace, including malformed IDs that must
/// be rejected rather than interpreted as an endpoint or default capture.
pub fn is_application_source(source_id: &str) -> bool {
    source_id.starts_with(application::SOURCE_PREFIX)
}

pub struct CaptureStream {
    #[cfg(windows)]
    inner: platform::Stream,
}

impl CaptureStream {
    /// Starts capture of a render endpoint or a selected application's process
    /// tree. An application ID pins one process lifetime; exit is an error and
    /// never falls back to capturing the computer's other sounds.
    /// `default` resolves when
    /// capture starts; changing the system default later does not silently move
    /// this session. Restart capture to use the newly selected endpoint.
    pub fn start(config: CaptureConfig) -> Result<Self> {
        config.validate()?;
        #[cfg(windows)]
        {
            Ok(Self {
                inner: platform::Stream::start(config)?,
            })
        }
        #[cfg(not(windows))]
        {
            Err(CaptureError::UnsupportedPlatform)
        }
    }

    /// `Ok(None)` means no chunk arrived before the timeout. A zero duration is
    /// a poll. Worker failures are reported as errors, not as silent PCM.
    pub fn read_timeout(&self, timeout: Duration) -> Result<Option<AudioChunk>> {
        #[cfg(windows)]
        {
            self.inner.read_timeout(timeout)
        }
        #[cfg(not(windows))]
        {
            let _ = timeout;
            Err(CaptureError::UnsupportedPlatform)
        }
    }

    /// Identifies the endpoint actually opened by the COM capture worker.
    pub fn info(&self) -> Result<CaptureInfo> {
        #[cfg(windows)]
        {
            Ok(self.inner.info.clone())
        }
        #[cfg(not(windows))]
        {
            Err(CaptureError::UnsupportedPlatform)
        }
    }

    /// Sends a command to the COM worker; never touches COM from the caller's
    /// thread. Disabling restores only a mute change still owned by this session.
    pub fn set_local_mute(&self, enabled: bool) -> Result<()> {
        #[cfg(windows)]
        {
            self.inner.set_local_mute(enabled)
        }
        #[cfg(not(windows))]
        {
            let _ = enabled;
            Err(CaptureError::UnsupportedPlatform)
        }
    }

    /// Stops capture and restores an owned local mute change. Idempotent.
    pub fn stop(&mut self) -> Result<()> {
        #[cfg(windows)]
        {
            self.inner.stop()
        }
        #[cfg(not(windows))]
        {
            Err(CaptureError::UnsupportedPlatform)
        }
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        #[cfg(windows)]
        let _ = self.inner.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unbounded_queues_and_invalid_sample_rates() {
        let mut config = CaptureConfig {
            queue_capacity: 0,
            ..CaptureConfig::default()
        };
        assert!(config.validate().is_err());
        config.queue_capacity = 65;
        assert!(config.validate().is_err());
        config.queue_capacity = 8;
        config.target_sample_rate = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn application_mode_rejects_malformed_identity_and_endpoint_mute() {
        let mut config = CaptureConfig {
            source_id: "process:42:100".into(),
            ..CaptureConfig::default()
        };
        assert!(config.validate().is_ok());
        config.mute_local = true;
        assert!(matches!(
            config.validate(),
            Err(CaptureError::ApplicationMuteUnsupported)
        ));
        config.mute_local = false;
        config.source_id = "process:42".into();
        assert!(matches!(
            config.validate(),
            Err(CaptureError::InvalidConfig(_))
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_never_claims_capture_success() {
        assert!(matches!(
            enumerate_sources(),
            Err(CaptureError::UnsupportedPlatform)
        ));
        assert!(matches!(
            enumerate_applications(),
            Err(CaptureError::UnsupportedPlatform)
        ));
        assert!(matches!(
            CaptureStream::start(CaptureConfig::default()),
            Err(CaptureError::UnsupportedPlatform)
        ));
    }
}
