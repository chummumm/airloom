//! Allowlisted session observations. No device identity, address, credentials,
//! raw plist, or authentication material belongs in this export.
use crate::{discovery::feature_bits, timing::TimingDiagnostics, Device};
use plist::Value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SenderDiagnostics {
    pub receiver: ReceiverDiagnostics,
    pub stream: StreamDiagnostics,
    pub timing: TimingDiagnostics,
    pub transport: TransportDiagnostics,
    /// A local, fixed stage name; never receiver-supplied error text.
    pub last_failure_stage: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiverDiagnostics {
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub airplay_version: Option<String>,
    /// Hex preserves all feature bits when a JSON consumer uses f64 numbers.
    pub features_hex: String,
    pub group_leader: Option<bool>,
    pub has_stereo_pair: bool,
    pub has_parent_group: bool,
    /// Receiver-advertised capabilities are advisory, not playback evidence.
    pub realtime_formats_hex: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamDiagnostics {
    pub protocol: String,
    pub stream_type: u16,
    pub codec: String,
    pub audio_format_hex: String,
    pub sample_rate: u32,
    pub sample_bits: u8,
    pub channels: u8,
    pub frames_per_packet: u16,
    pub requested_latency_ms: u64,
    pub receiver_latency_min_frames: Option<u32>,
    pub receiver_latency_max_frames: Option<u32>,
    pub effective_latency_frames: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransportDiagnostics {
    pub audio_packets_sent: u64,
    pub non_silent_audio_packets_sent: u64,
    /// Peak of the most recently sent PCM packet, before receiver volume.
    pub last_audio_peak_dbfs: Option<f32>,
    pub sync_packets_sent: u64,
    pub control_packets_received: u64,
    pub retransmission_requests: u64,
    pub retransmissions_sent: u64,
    pub feedback_requests: u64,
    pub feedback_responses: u64,
    pub feedback_last_status: Option<u16>,
    pub feedback_last_response_age_ms: Option<u64>,
    /// None means omitted. An empty list does not establish playback failure.
    pub feedback_reported_streams: Option<u64>,
    pub event_requests: u64,
    pub event_responses: u64,
}

/// Only protocol model/version tokens are retained. Device names, arbitrary
/// Unicode text, header values, and binary data are never diagnostic metadata.
fn metadata_token(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".,_-".contains(&b)))
    .then(|| value.to_owned())
}

fn version(value: &str) -> Option<String> {
    value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_digit)
        .then(|| metadata_token(value))
        .flatten()
}

fn device_version(device: &Device, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| device.txt.get(*key).and_then(|s| version(s)))
}

pub(crate) fn receiver_diagnostics(
    device: &Device,
    realtime_formats: Option<u64>,
) -> ReceiverDiagnostics {
    ReceiverDiagnostics {
        model: device.model.as_deref().and_then(metadata_token),
        os_version: device_version(device, &["osvers", "ov"]),
        airplay_version: device_version(device, &["srcvers", "vs"]),
        features_hex: format!("0x{:016x}", device.features),
        group_leader: device.txt.get("igl").and_then(|s| match s.as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        }),
        has_stereo_pair: device.txt.get("tsid").is_some_and(|s| !s.is_empty()),
        has_parent_group: device.txt.get("pgid").is_some_and(|s| !s.is_empty()),
        realtime_formats_hex: realtime_formats.map(|mask| format!("0x{mask:016x}")),
    }
}

/// GET /info can supply a version absent from the discovery TXT. Preserve an
/// explicit discovery version, including older OS versions, instead of letting
/// a sourceVersion fallback accidentally change the clock-role decision.
pub(crate) fn merge_receiver_info(device: &mut Device, info: &Value) -> Option<u64> {
    let info = info.as_dictionary()?;
    if let Some(model) = info
        .get("model")
        .and_then(Value::as_string)
        .and_then(metadata_token)
    {
        device.model = Some(model);
    }
    if let Some(features) = info
        .get("features")
        .and_then(|value| integer_bits(value).or_else(|| value.as_string().and_then(feature_bits)))
    {
        device.features = features;
    }
    for (aliases, info_keys, canonical) in [
        (["osvers", "ov"], ["osVersion", "osvers"], "osvers"),
        (["srcvers", "vs"], ["sourceVersion", "srcvers"], "srcvers"),
    ] {
        if device_version(device, &aliases).is_none() {
            if let Some(value) = info_keys
                .iter()
                .find_map(|key| info.get(key).and_then(Value::as_string).and_then(version))
            {
                device.txt.insert(canonical.into(), value);
            }
        }
    }
    if let Some(extended) = info
        .get("supportedAudioFormatsExtended")
        .and_then(Value::as_dictionary)
        .and_then(|formats| formats.get("audioStream"))
        .and_then(Value::as_array)
    {
        let mut mask = 0;
        for bit in extended {
            // A malformed table is unknown, not an empty capability set.
            let bit = bit.as_unsigned_integer()?;
            if bit < 64 {
                mask |= 1u64 << bit;
            }
        }
        return Some(mask);
    }
    info.get("supportedFormats")
        .and_then(Value::as_dictionary)
        .and_then(|formats| formats.get("audioStream"))
        .and_then(integer_bits)
}

fn integer_bits(value: &Value) -> Option<u64> {
    value
        .as_unsigned_integer()
        .or_else(|| value.as_signed_integer().map(|bits| bits as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::Dictionary;

    #[test]
    fn info_fills_missing_versions_without_replacing_explicit_older_discovery() {
        let mut device = Device::manual("192.0.2.1".parse().unwrap(), 7000);
        let mut fields = Dictionary::new();
        fields.insert("model".into(), Value::String("AudioAccessory5,1".into()));
        fields.insert("osVersion".into(), Value::String("27.0".into()));
        fields.insert("sourceVersion".into(), Value::String("980.2".into()));
        let value = Value::Dictionary(fields);
        merge_receiver_info(&mut device, &value);
        assert_eq!(device.txt["osvers"], "27.0");
        assert_eq!(device.txt["srcvers"], "980.2");
        device.txt.remove("osvers");
        device.txt.insert("ov".into(), "26.4".into());
        merge_receiver_info(&mut device, &value);
        assert!(!device.txt.contains_key("osvers"));
        assert_eq!(
            receiver_diagnostics(&device, None).os_version.as_deref(),
            Some("26.4")
        );
    }

    #[test]
    fn diagnostics_export_allowlist_does_not_include_identity_or_raw_values() {
        let mut device = Device::manual("192.0.2.117".parse().unwrap(), 7000);
        device.name = "personal room name".into();
        device.id = "AA:BB:CC:DD:EE:FF".into();
        device
            .txt
            .insert("serialNumber".into(), "private-serial".into());
        device.txt.insert("pk".into(), "private-key".into());
        device.txt.insert("tsid".into(), "private-stereo-id".into());
        device
            .txt
            .insert("osvers".into(), "27.0\r\nprivate-header".into());
        device.model = Some("invalid model / private value".into());
        let diagnostic = receiver_diagnostics(&device, None);
        assert!(diagnostic.model.is_none() && diagnostic.os_version.is_none());
        assert!(diagnostic.has_stereo_pair);
        let json = serde_json::to_string(&diagnostic).unwrap();
        for private in [
            "192.0.2.117",
            "personal",
            "AA:BB",
            "private",
            "serialNumber",
            "tsid",
            "pk",
        ] {
            assert!(
                !json.contains(private),
                "unexpected private metadata: {private}"
            );
        }
    }

    #[test]
    fn extended_formats_override_legacy_without_truncating_high_bits() {
        let mut device = Device::manual("192.0.2.1".parse().unwrap(), 7000);
        let mut extended = Dictionary::new();
        extended.insert(
            "audioStream".into(),
            Value::Array(vec![Value::Integer(18.into()), Value::Integer(63.into())]),
        );
        let mut legacy = Dictionary::new();
        legacy.insert("audioStream".into(), Value::Integer(0x800.into()));
        let mut info = Dictionary::new();
        info.insert(
            "supportedAudioFormatsExtended".into(),
            Value::Dictionary(extended),
        );
        info.insert("supportedFormats".into(), Value::Dictionary(legacy));
        assert_eq!(
            merge_receiver_info(&mut device, &Value::Dictionary(info)),
            Some(0x8000_0000_0004_0000)
        );
    }
}
