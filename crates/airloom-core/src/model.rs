use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    #[default]
    Endpoint,
    Application,
}

impl SourceMode {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Endpoint => "endpoint",
            Self::Application => "application",
        }
    }
}

#[derive(Clone, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub model: String,
    pub address: String,
    pub password_required: bool,
    pub online: bool,
    pub remembered: bool,
    pub selected: bool,
    pub state: Phase,
    pub status_message: String,
    pub error: Option<AppError>,
    pub packets_sent: u64,
    pub timing_exchanges: u64,
    pub kind: &'static str,
    pub members: Vec<DeviceMemberView>,
    pub expected_members: usize,
    pub group_complete: bool,
}

#[derive(Clone, Serialize)]
pub struct DeviceMemberView {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub state: Phase,
    pub status_message: String,
}

#[derive(Clone, Serialize)]
pub struct SourceView {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub is_default: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
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
    pub device_name: String,
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
pub struct CaptureView {
    /// Actual captured signal; a generated test tone never changes this value.
    pub state: &'static str,
    pub source_id: String,
    pub source_name: String,
    pub message: String,
    pub frames_received: u64,
    pub chunks_received: u64,
    pub last_packet_ms: Option<u64>,
    pub silent_ms: u64,
    pub level: Levels,
}

impl Default for CaptureView {
    fn default() -> Self {
        Self {
            state: "idle",
            source_id: String::new(),
            source_name: String::new(),
            message: "连接后显示所选声音来源的真实声音。".into(),
            frames_received: 0,
            chunks_received: 0,
            last_packet_ms: None,
            silent_ms: 0,
            level: Levels::default(),
        }
    }
}

/// This is the only object the UI may copy for troubleshooting. Keep it
/// separate from Snapshot: normal UI state contains names and identifiers.
#[derive(Clone, Serialize)]
pub struct Diagnostics {
    pub schema_version: u8,
    pub app_version: &'static str,
    pub session_state: Phase,
    pub volume_percent: u8,
    pub mute_local: bool,
    pub test_tone_active: bool,
    pub capture: CaptureDiagnostics,
    pub receivers: Vec<ReceiverDiagnostics>,
}

#[derive(Clone, Serialize)]
pub struct CaptureDiagnostics {
    pub source_mode: SourceMode,
    pub state: &'static str,
    pub frames_received: u64,
    pub chunks_received: u64,
    pub last_packet_ms: Option<u64>,
    pub silent_ms: u64,
    pub level: Levels,
    pub buffer_ms: Option<f32>,
    pub dropped_frames: u64,
    pub underrun_frames: u64,
}

#[derive(Clone, Serialize)]
pub struct ReceiverDiagnostics {
    /// Position in the current selection, not a persistent device identity.
    pub slot: usize,
    pub state: Phase,
    pub session: airloom_airplay::SenderDiagnostics,
}

impl Diagnostics {
    pub fn from_snapshot(snapshot: &Snapshot, receivers: Vec<ReceiverDiagnostics>) -> Self {
        Self {
            schema_version: 1,
            app_version: env!("CARGO_PKG_VERSION"),
            session_state: snapshot.state,
            volume_percent: (snapshot.volume.clamp(0.0, 1.0) * 100.0).round() as u8,
            mute_local: snapshot.mute_local,
            test_tone_active: snapshot.test_tone_active,
            capture: CaptureDiagnostics {
                source_mode: snapshot.source_mode,
                state: snapshot.capture.state,
                frames_received: snapshot.capture.frames_received,
                chunks_received: snapshot.capture.chunks_received,
                last_packet_ms: snapshot.capture.last_packet_ms,
                silent_ms: snapshot.capture.silent_ms,
                level: snapshot.capture.level.clone(),
                buffer_ms: snapshot.metrics.buffer_ms,
                dropped_frames: snapshot.metrics.dropped_frames,
                underrun_frames: snapshot.metrics.underrun_frames,
            },
            receivers,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub version: u8,
    pub revision: u64,
    pub state: Phase,
    pub status_message: String,
    pub devices: Vec<DeviceView>,
    pub sources: Vec<SourceView>,
    pub source_mode: SourceMode,
    pub sources_refreshing: bool,
    pub source_message: String,
    pub selected_device_id: String,
    pub selected_device_ids: Vec<String>,
    pub selected_source_id: String,
    pub volume: f32,
    pub mute_local: bool,
    pub level: Levels,
    pub waveform: Vec<f32>,
    pub error: Option<AppError>,
    pub auth: Option<AuthPrompt>,
    pub metrics: Metrics,
    pub elapsed_seconds: u64,
    pub capture: CaptureView,
    pub test_tone_active: bool,
    pub test_tone_remaining_ms: u32,
    pub diagnostics: Option<Diagnostics>,
}

impl Default for Snapshot {
    fn default() -> Self {
        let mut snapshot = Self {
            version: 1,
            revision: 0,
            state: Phase::Idle,
            status_message: "选择一个或多个设备，让声音流动起来。".into(),
            devices: Vec::new(),
            sources: Vec::new(),
            source_mode: SourceMode::Endpoint,
            sources_refreshing: false,
            source_message: "选择电脑正在播放声音的输出设备。".into(),
            selected_device_id: String::new(),
            selected_device_ids: Vec::new(),
            selected_source_id: "default".into(),
            volume: 0.5,
            mute_local: false,
            level: Levels::default(),
            waveform: vec![0.0; 64],
            error: None,
            auth: None,
            metrics: Metrics::default(),
            elapsed_seconds: 0,
            capture: CaptureView::default(),
            test_tone_active: false,
            test_tone_remaining_ms: 0,
            diagnostics: None,
        };
        snapshot.diagnostics = Some(Diagnostics::from_snapshot(&snapshot, Vec::new()));
        snapshot
    }
}

// Deliberately no Debug: authentication values must not appear in diagnostics.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Refresh {},
    RefreshSources {},
    SelectSourceMode {
        mode: SourceMode,
    },
    SelectDevice {
        device_id: String,
    },
    ToggleDevice {
        device_id: String,
        #[serde(default)]
        selected: Option<bool>,
    },
    SelectSource {
        source_id: String,
    },
    Connect {
        #[serde(default)]
        device_id: Option<String>,
    },
    Pair {
        #[serde(default)]
        device_id: Option<String>,
        #[serde(default)]
        request_id: Option<u64>,
    },
    Disconnect {
        #[serde(default)]
        device_id: Option<String>,
        #[serde(default)]
        request_id: Option<u64>,
    },
    SetVolume {
        volume: f32,
    },
    SetMuteLocal {
        enabled: bool,
    },
    TestTone {
        #[serde(default)]
        enabled: Option<bool>,
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
            Self::SelectDevice { device_id }
            | Self::ToggleDevice { device_id, .. }
            | Self::ForgetCredentials { device_id } => valid_identifier(device_id, 1024),
            Self::SelectSource { source_id } => valid_identifier(source_id, 4096),
            Self::Authenticate {
                request_id, value, ..
            } => {
                *request_id != 0
                    && !value.is_empty()
                    && value.len() <= 1024
                    && !value.contains('\0')
            }
            Self::Pair {
                request_id,
                device_id,
            }
            | Self::Disconnect {
                request_id,
                device_id,
            } => {
                *request_id != Some(0)
                    && device_id
                        .as_ref()
                        .is_none_or(|id| valid_identifier(id, 1024))
            }
            Self::Connect { device_id } => device_id
                .as_ref()
                .is_none_or(|id| valid_identifier(id, 1024)),
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
    fn copied_diagnostics_include_signal_and_session_progress_but_no_ui_identity() {
        let mut snapshot = Snapshot {
            selected_device_id: "private-device-identity".into(),
            selected_device_ids: vec!["private-device-identity".into()],
            selected_source_id: "private-endpoint-identity".into(),
            status_message: "private receiver response".into(),
            volume: 0.37,
            state: Phase::Streaming,
            auth: Some(AuthPrompt {
                request_id: 7,
                kind: "password",
                device_id: "private-device-identity".into(),
                device_name: "private room name".into(),
                message: "private authentication text".into(),
            }),
            ..Snapshot::default()
        };
        snapshot.capture.source_id = "private-endpoint-identity".into();
        snapshot.capture.source_name = "private microphone name".into();
        snapshot.capture.message = "private capture error".into();
        snapshot.capture.state = "signal";
        snapshot.capture.frames_received = 44_100;
        snapshot.capture.level.left = 0.25;
        let mut session = airloom_airplay::SenderDiagnostics::default();
        session.transport.non_silent_audio_packets_sent = 125;
        let diagnostic = Diagnostics::from_snapshot(
            &snapshot,
            vec![ReceiverDiagnostics {
                slot: 1,
                state: Phase::Streaming,
                session,
            }],
        );
        let json = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(json["volume_percent"], 37);
        assert_eq!(json["capture"]["frames_received"], 44_100);
        assert_eq!(
            json["receivers"][0]["session"]["transport"]["non_silent_audio_packets_sent"],
            125
        );
        let text = json.to_string();
        for excluded in [
            "private",
            "password",
            "device_id",
            "source_name",
            "request_id",
        ] {
            assert!(
                !text.contains(excluded),
                "unexpected export field: {excluded}"
            );
        }
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
    fn multi_device_commands_keep_legacy_forms_and_validate_optional_targets() {
        for json in [
            r#"{"type":"connect"}"#,
            r#"{"type":"disconnect"}"#,
            r#"{"type":"select_device","device_id":"speaker-one"}"#,
            r#"{"type":"toggle_device","device_id":"speaker-two","selected":true}"#,
            r#"{"type":"toggle_device","device_id":"speaker-two"}"#,
            r#"{"type":"connect","device_id":"speaker-two"}"#,
            r#"{"type":"disconnect","device_id":"speaker-two"}"#,
            r#"{"type":"pair","device_id":"speaker-two","request_id":42}"#,
            r#"{"type":"test_tone"}"#,
            r#"{"type":"test_tone","enabled":false}"#,
        ] {
            assert!(serde_json::from_str::<Command>(json).unwrap().validate());
        }
        for json in [
            r#"{"type":"connect","device_id":""}"#,
            r#"{"type":"disconnect","device_id":"bad\u0000id"}"#,
            r#"{"type":"pair","request_id":0}"#,
        ] {
            assert!(!serde_json::from_str::<Command>(json).unwrap().validate());
        }
        assert!(serde_json::from_str::<Command>(
            r#"{"type":"toggle_device","device_id":"one","selected":"yes"}"#
        )
        .is_err());
    }

    #[test]
    fn visual_envelope_is_flat_only_for_silence() {
        assert_eq!(waveform(&[0.0; 256]), vec![0.0; 64]);
        assert_eq!(waveform(&[0.25; 256]), vec![0.25; 64]);
    }
}
