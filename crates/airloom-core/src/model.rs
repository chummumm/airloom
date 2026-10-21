use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub model: String,
    pub address: String,
    pub password_required: bool,
    pub online: bool,
    pub remembered: bool,
}

#[derive(Clone, Serialize)]
pub struct SourceView {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub is_default: bool,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Discovering,
    Connecting,
    AuthRequired,
    Streaming,
    Reconnecting,
    Error,
}

#[derive(Clone, Serialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Serialize)]
pub struct AuthPrompt {
    /// Identifies this exact prompt; stale dialog responses must be ignored.
    pub request_id: u64,
    pub kind: &'static str,
    pub device_id: String,
    pub message: String,
}

#[derive(Clone, Default, Serialize)]
pub struct Levels {
    pub left: f32,
    pub right: f32,
}

#[derive(Clone, Default, Serialize)]
pub struct Metrics {
    pub packets_sent: u64,
    pub sample_rate: u32,
    pub buffer_ms: Option<f32>,
    pub latency_ms: Option<f32>,
    pub dropped_frames: u64,
    pub underrun_frames: u64,
}

#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub version: u8,
    pub revision: u64,
    pub state: Phase,
    pub status_message: String,
    pub devices: Vec<DeviceView>,
    pub sources: Vec<SourceView>,
    pub selected_device_id: String,
    pub selected_source_id: String,
    pub volume: f32,
    pub mute_local: bool,
    pub level: Levels,
    pub waveform: Vec<f32>,
    pub error: Option<AppError>,
    pub auth: Option<AuthPrompt>,
    pub metrics: Metrics,
    pub elapsed_seconds: u64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            state: Phase::Idle,
            status_message: "选择一个设备，让声音流动起来。".into(),
            devices: Vec::new(),
            sources: Vec::new(),
            selected_device_id: String::new(),
            selected_source_id: "default".into(),
            volume: 0.5,
            mute_local: false,
            level: Levels::default(),
            waveform: vec![0.0; 64],
            error: None,
            auth: None,
            metrics: Metrics::default(),
            elapsed_seconds: 0,
        }
    }
}

// Deliberately no Debug: authentication values must not appear in diagnostics.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Refresh {},
    SelectDevice {
        device_id: String,
    },
    SelectSource {
        source_id: String,
    },
    Connect {},
    Pair {
        #[serde(default)]
        request_id: Option<u64>,
    },
    Disconnect {
        #[serde(default)]
        request_id: Option<u64>,
    },
    SetVolume {
        volume: f32,
    },
    SetMuteLocal {
        enabled: bool,
    },
    Authenticate {
        request_id: u64,
        value: zeroize::Zeroizing<String>,
        remember: bool,
    },
    ForgetCredentials {
        device_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandError {
    Invalid,
    QueueFull,
    Stopped,
}

impl Command {
    pub fn validate(&self) -> bool {
        match self {
            Self::SetVolume { volume } => volume.is_finite() && (0.0..=1.0).contains(volume),
            Self::SelectDevice { device_id } | Self::ForgetCredentials { device_id } => {
                valid_identifier(device_id, 1024)
            }
            Self::SelectSource { source_id } => valid_identifier(source_id, 4096),
            Self::Authenticate {
                request_id, value, ..
            } => {
                *request_id != 0
                    && !value.is_empty()
                    && value.len() <= 1024
                    && !value.contains('\0')
            }
            Self::Pair { request_id } | Self::Disconnect { request_id } => *request_id != Some(0),
            _ => true,
        }
    }
}

fn valid_identifier(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= maximum_bytes && !value.chars().any(char::is_control)
}

pub fn pcm_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&sample| {
            if !sample.is_finite() {
                0
            } else if sample <= -1.0 {
                i16::MIN
            } else {
                (sample.min(1.0) * i16::MAX as f32).round() as i16
            }
        })
        .collect()
}

/// A 64-bin peak envelope from actual stereo samples. A silent frame stays flat.
pub fn waveform(samples: &[f32]) -> Vec<f32> {
    let frames = samples.len() / 2;
    (0..64)
        .map(|bin| {
            let start = bin * frames / 64;
            let end = ((bin + 1) * frames / 64).min(frames);
            (start..end)
                .map(|frame| samples[2 * frame].abs().max(samples[2 * frame + 1].abs()))
                .filter(|sample| sample.is_finite())
                .fold(0.0_f32, f32::max)
                .min(1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_clips_and_sanitizes_invalid_samples() {
        assert_eq!(
            pcm_i16(&[-2.0, -1.0, 0.0, 1.0, 2.0, f32::NAN, f32::INFINITY]),
            vec![-32768, -32768, 0, 32767, 32767, 0, 0]
        );
    }

    #[test]
    fn snapshots_cannot_contain_an_authentication_value() {
        let json = serde_json::to_value(Snapshot::default()).unwrap();
        assert_eq!(json["version"], 1);
        assert!(json.get("password").is_none());
        assert!(json.get("credentials").is_none());
        assert!(json["auth"].is_null());
    }

    #[test]
    fn commands_reject_typos_and_out_of_range_volume() {
        for command in ["connect", "refresh", "pair", "disconnect"] {
            let json = format!(r#"{{"type":"{command}","pasword":"x"}}"#);
            assert!(serde_json::from_str::<Command>(&json).is_err(), "{command}");
            let valid = format!(r#"{{"type":"{command}"}}"#);
            assert!(serde_json::from_str::<Command>(&valid).unwrap().validate());
        }
        let invalid: Command = serde_json::from_str(r#"{"type":"set_volume","volume":2}"#).unwrap();
        assert!(!invalid.validate());
    }

    #[test]
    fn authentication_requires_a_prompt_identity_and_sanitized_identifiers() {
        assert!(serde_json::from_str::<Command>(
            r#"{"type":"authenticate","value":"secret","remember":false}"#
        )
        .is_err());
        let valid: Command = serde_json::from_str(
            r#"{"type":"authenticate","request_id":42,"value":"secret","remember":false}"#,
        )
        .unwrap();
        assert!(valid.validate());
        let invalid: Command =
            serde_json::from_str(r#"{"type":"select_source","source_id":"bad\u0000id"}"#).unwrap();
        assert!(!invalid.validate());
    }

    #[test]
    fn visual_envelope_is_flat_only_for_silence() {
        assert_eq!(waveform(&[0.0; 256]), vec![0.0; 64]);
        assert_eq!(waveform(&[0.25; 256]), vec![0.25; 64]);
    }
}
