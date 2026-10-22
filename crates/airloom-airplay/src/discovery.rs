use crate::{Error, Result};
use mdns_sd::{DaemonEvent, Receiver, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::IpAddr, time::Duration};

const AIRPLAY: &str = "_airplay._tcp.local.";
const RAOP: &str = "_raop._tcp.local.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub host: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub model: Option<String>,
    pub features: u64,
    pub requires_password: bool,
    pub requires_pin: bool,
    pub txt: BTreeMap<String, String>,
}

/// A native HomePod stereo-pair candidate advertised by the receiver. This is
/// discovery evidence, not proof that the pair accepts our streaming protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeStereoInfo {
    pub id: String,
    pub name: Option<String>,
}

impl Device {
    pub fn manual(address: IpAddr, port: u16) -> Self {
        Self {
            id: format!("manual:{address}:{port}"),
            name: address.to_string(),
            host: address.to_string(),
            addresses: vec![address],
            port,
            model: None,
            features: 0,
            requires_password: false,
            requires_pin: false,
            txt: BTreeMap::new(),
        }
    }

    pub fn native_stereo(&self) -> Option<NativeStereoInfo> {
        if !self.model.as_deref()?.starts_with("AudioAccessory") {
            return None;
        }
        // gid/pgid describe dynamic playback groups as well as persistent
        // groups; igl is not a unique stereo leader. Never infer a pair or a
        // left/right role from them, the public name, or the opaque tsm field.
        let id = stereo_uuid(self.txt.get("tsid")?)?;
        let name = self.txt.get("gpn").filter(|name| {
            !name.trim().is_empty()
                && name.chars().count() <= 128
                && !name.chars().any(char::is_control)
        });
        Some(NativeStereoInfo {
            id,
            name: name.cloned(),
        })
    }
}

fn stereo_uuid(value: &str) -> Option<String> {
    if value.len() != 36 {
        return None;
    }
    let mut nonzero = false;
    for (index, byte) in value.bytes().enumerate() {
        if [8, 13, 18, 23].contains(&index) {
            if byte != b'-' {
                return None;
            }
        } else {
            if !byte.is_ascii_hexdigit() {
                return None;
            }
            nonzero |= byte != b'0';
        }
    }
    nonzero.then(|| value.to_ascii_lowercase())
}

/// A single persistent DNS-SD subscription. The daemon handles DNS cache TTLs
/// and protocol queries; Airloom adds no polling or periodic rescan timer.
/// Dropping this value stops both browses and its daemon. Manual refresh keeps
/// the same DNS cache and catalog while requesting fresh service queries.
pub struct DiscoveryWatcher {
    daemon: ServiceDaemon,
    airplay: Receiver<ServiceEvent>,
    raop: Receiver<ServiceEvent>,
    retired_airplay: Option<Receiver<ServiceEvent>>,
    retired_raop: Option<Receiver<ServiceEvent>>,
    monitor: Receiver<DaemonEvent>,
    catalog: DiscoveryCatalog,
}

impl DiscoveryWatcher {
    pub fn start() -> Result<Self> {
        let daemon = ServiceDaemon::new().map_err(discovery_error)?;
        let subscriptions = (|| {
            Ok((
                daemon.browse(AIRPLAY).map_err(discovery_error)?,
                daemon.browse(RAOP).map_err(discovery_error)?,
                daemon.monitor().map_err(discovery_error)?,
            ))
        })();
        let (airplay, raop, monitor) = match subscriptions {
            Ok(receivers) => receivers,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(error);
            }
        };
        Ok(Self {
            daemon,
            airplay,
            raop,
            retired_airplay: None,
            retired_raop: None,
            monitor,
            catalog: DiscoveryCatalog::default(),
        })
    }

    /// Re-query both service types without temporarily forgetting receivers or
    /// their AirPlay-only group metadata. Refreshes still being drained coalesce.
    pub fn refresh(&mut self) -> Result<()> {
        if self.retired_airplay.is_some() || self.retired_raop.is_some() {
            return Ok(());
        }
        // Queue both stops before either new browse. mdns-sd preserves its DNS
        // cache, cancels the old query retries and seals each old event stream
        // with SearchStopped before replaying cached records to the new stream.
        self.daemon.stop_browse(AIRPLAY).map_err(discovery_error)?;
        self.daemon.stop_browse(RAOP).map_err(discovery_error)?;
        let airplay = self.daemon.browse(AIRPLAY).map_err(discovery_error)?;
        let raop = self.daemon.browse(RAOP).map_err(discovery_error)?;
        self.retired_airplay = Some(std::mem::replace(&mut self.airplay, airplay));
        self.retired_raop = Some(std::mem::replace(&mut self.raop, raop));
        Ok(())
    }

    /// Cancel-safe: return the next changed physical-device snapshot. A removed
    /// RAOP advertisement does not remove its still-advertised AirPlay device.
    pub async fn next(&mut self) -> Result<Vec<Device>> {
        loop {
            let draining = self.retired_airplay.is_some() || self.retired_raop.is_some();
            // Finish the old streams before replaying the new cache. Otherwise
            // a queued removal could be lost, or erase a newer resolved record.
            let (service_type, event, retired) = tokio::select! {
                event = self.airplay.recv_async(), if !draining =>
                    (AIRPLAY, event.map_err(discovery_error)?, false),
                event = self.raop.recv_async(), if !draining =>
                    (RAOP, event.map_err(discovery_error)?, false),
                event = retired_event(&self.retired_airplay) => (AIRPLAY, event?, true),
                event = retired_event(&self.retired_raop) => (RAOP, event?, true),
                event = self.monitor.recv_async() => {
                    if let DaemonEvent::Error(error) = event.map_err(discovery_error)? {
                        return Err(discovery_error(error));
                    }
                    continue;
                },
            };
            if matches!(event, ServiceEvent::SearchStopped(_)) {
                if retired {
                    if service_type == AIRPLAY {
                        self.retired_airplay = None;
                    } else {
                        self.retired_raop = None;
                    }
                    continue;
                }
                return Err(Error::Discovery(
                    "device discovery subscription stopped".into(),
                ));
            }
            if let Some(devices) = self.catalog.apply(service_type, event) {
                return Ok(devices);
            }
        }
    }

    pub fn devices(&self) -> &[Device] {
        &self.catalog.devices
    }
}

async fn retired_event(receiver: &Option<Receiver<ServiceEvent>>) -> Result<ServiceEvent> {
    match receiver {
        Some(receiver) => receiver.recv_async().await.map_err(discovery_error),
        None => std::future::pending().await,
    }
}

impl Drop for DiscoveryWatcher {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(AIRPLAY);
        let _ = self.daemon.stop_browse(RAOP);
        let _ = self.daemon.shutdown();
    }
}

fn discovery_error(error: impl ToString) -> Error {
    Error::Discovery(error.to_string())
}

#[derive(Default)]
struct DiscoveryCatalog {
    // Keep each advertisement separately so updates replace their old TXT and
    // addresses, while removes retire only the corresponding service source.
    services: BTreeMap<(String, String), Device>,
    devices: Vec<Device>,
}

impl DiscoveryCatalog {
    fn apply(&mut self, service_type: &str, event: ServiceEvent) -> Option<Vec<Device>> {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                self.services.insert(
                    (
                        service_type.into(),
                        info.get_fullname().to_ascii_lowercase(),
                    ),
                    from_service(&info),
                );
            }
            ServiceEvent::ServiceRemoved(kind, fullname)
                if kind.eq_ignore_ascii_case(service_type) =>
            {
                self.services
                    .remove(&(service_type.into(), fullname.to_ascii_lowercase()));
            }
            _ => return None,
        }
        let mut merged = BTreeMap::<String, Device>::new();
        // AirPlay's endpoint and overlapping TXT win, independently of event
        // arrival order. Missing fields may still come from the RAOP service.
        for kind in [RAOP, AIRPLAY] {
            for ((service_type, _), device) in &self.services {
                if service_type != kind {
                    continue;
                }
                let previous = merged.insert(device.id.clone(), device.clone());
                if let Some(previous) = previous {
                    let current = merged.get_mut(&device.id).expect("just inserted");
                    current.addresses.extend(previous.addresses);
                    current.addresses.sort_by_key(|ip| (!ip.is_ipv4(), *ip));
                    current.addresses.dedup();
                    let mut txt = previous.txt;
                    txt.extend(current.txt.clone());
                    current.txt = txt;
                    current.requires_password |= previous.requires_password;
                    current.requires_pin |= previous.requires_pin;
                }
            }
        }
        let devices: Vec<_> = merged.into_values().collect();
        if devices == self.devices {
            return None;
        }
        self.devices = devices.clone();
        Some(devices)
    }
}

pub async fn discover(duration: Duration) -> Result<Vec<Device>> {
    if duration > Duration::from_secs(30) {
        return Err(Error::InvalidArgument(
            "discovery window must be at most 30 seconds".into(),
        ));
    }
    let mut watcher = DiscoveryWatcher::start()?;
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        match tokio::time::timeout_at(deadline, watcher.next()).await {
            Ok(result) => {
                result?;
            }
            Err(_) => return Ok(watcher.devices().to_vec()),
        }
    }
}

pub(crate) fn feature_bits(value: &str) -> Option<u64> {
    let mut halves = value.split(',');
    let number = |value: &str| {
        let value = value.trim();
        u64::from_str_radix(
            value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
                .unwrap_or(value),
            16,
        )
        .ok()
    };
    let low = number(halves.next()?)?;
    let high = match halves.next() {
        Some(value) => Some(number(value)?),
        None => None,
    };
    if halves.next().is_some() {
        return None;
    }
    match high {
        Some(high) if high <= u32::MAX as u64 && low <= u32::MAX as u64 => Some((high << 32) | low),
        Some(_) => None,
        None => Some(low),
    }
}

fn from_service(info: &ServiceInfo) -> Device {
    let txt: BTreeMap<String, String> = info
        .get_properties()
        .iter()
        .map(|p| (p.key().to_ascii_lowercase(), p.val_str().to_owned()))
        .collect();
    let fullname = info.get_fullname();
    let instance = fullname.split("._").next().unwrap_or(fullname);
    let (raop_id, name) = instance
        .split_once('@')
        .map(|(id, name)| (Some(id), name))
        .unwrap_or((None, instance));
    let id = txt
        .get("deviceid")
        .map(String::as_str)
        .or(raop_id)
        .map(|id| id.replace(':', "").to_ascii_uppercase())
        .unwrap_or_else(|| info.get_hostname().to_ascii_lowercase());
    let features = txt
        .get("features")
        .or_else(|| txt.get("ft"))
        .and_then(|s| feature_bits(s))
        .unwrap_or(0);
    let flags = txt
        .get("flags")
        .or_else(|| txt.get("sf"))
        .and_then(|s| feature_bits(s))
        .unwrap_or(0);
    let mut addresses: Vec<_> = info.get_addresses().iter().copied().collect();
    addresses.sort_by_key(|a| (!a.is_ipv4(), *a));
    Device {
        id,
        name: name.to_owned(),
        host: info.get_hostname().to_owned(),
        addresses,
        port: info.get_port(),
        model: txt.get("model").or_else(|| txt.get("am")).cloned(),
        features,
        requires_password: txt
            .get("pw")
            .is_some_and(|s| s.eq_ignore_ascii_case("true")),
        requires_pin: flags & 8 != 0,
        txt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const STEREO: &str = "EAFA36AA-9785-54B2-A537-D9EE2A55CF1C";

    fn service(kind: &str, ip: &str, properties: &[(&str, &str)]) -> ServiceInfo {
        let name = if kind == RAOP {
            "001122334455@Speaker"
        } else {
            "Speaker"
        };
        ServiceInfo::new(
            kind,
            name,
            "speaker.local.",
            ip,
            if kind == RAOP { 5000 } else { 7000 },
            properties,
        )
        .unwrap()
    }

    #[test]
    fn apple_features_are_low_word_first() {
        assert_eq!(
            feature_bits("0x12345678,0x01234567"),
            Some(0x0123456712345678)
        );
        assert_eq!(feature_bits("0x100000000,1"), None);
        assert_eq!(feature_bits("0x12,wrong"), None);
    }

    #[test]
    fn manual_refresh_preserves_catalog_when_raop_replays_before_airplay() {
        let mut watcher = DiscoveryWatcher::start().unwrap();
        let airplay = service(
            AIRPLAY,
            "192.0.2.10",
            &[
                ("deviceid", "00:11:22:33:44:55"),
                ("model", "AudioAccessory5,1"),
                ("tsid", STEREO),
            ],
        );
        let raop = service(RAOP, "192.0.2.10", &[("am", "AudioAccessory5,1")]);
        watcher
            .catalog
            .apply(AIRPLAY, ServiceEvent::ServiceResolved(airplay.clone()));
        watcher
            .catalog
            .apply(RAOP, ServiceEvent::ServiceResolved(raop.clone()));
        let before = watcher.devices().to_vec();

        watcher.refresh().unwrap();
        assert_eq!(watcher.devices(), before);
        assert!(watcher.retired_airplay.is_some() && watcher.retired_raop.is_some());
        assert!(watcher
            .catalog
            .apply(RAOP, ServiceEvent::ServiceResolved(raop.clone()))
            .is_none());
        assert_eq!(
            watcher.devices()[0].native_stereo().unwrap().id,
            STEREO.to_ascii_lowercase()
        );
        assert_eq!(watcher.devices()[0].port, 7000);
        assert!(watcher
            .catalog
            .apply(AIRPLAY, ServiceEvent::ServiceResolved(airplay.clone()))
            .is_none());

        // Keeping the catalog for refresh must not pin vanished services.
        watcher.catalog.apply(
            AIRPLAY,
            ServiceEvent::ServiceRemoved(AIRPLAY.into(), airplay.get_fullname().into()),
        );
        watcher.catalog.apply(
            RAOP,
            ServiceEvent::ServiceRemoved(RAOP.into(), raop.get_fullname().into()),
        );
        assert!(watcher.devices().is_empty());
    }

    #[tokio::test]
    async fn refreshed_subscriptions_retire_without_stopping_discovery() {
        let mut watcher = DiscoveryWatcher::start().unwrap();
        watcher.refresh().unwrap();
        // Rapid repeated clicks coalesce while the prior streams finish.
        watcher.refresh().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while watcher.retired_airplay.is_some() || watcher.retired_raop.is_some() {
            match tokio::time::timeout_at(deadline, watcher.next()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("refresh stopped the discovery stream: {error}"),
                Err(_) => break,
            }
        }
        assert!(watcher.retired_airplay.is_none() && watcher.retired_raop.is_none());
        // Cancellation while waiting for devices leaves the subscription usable.
        watcher.refresh().unwrap();
        assert!(watcher.retired_airplay.is_some() && watcher.retired_raop.is_some());
    }

    #[test]
    fn discovery_merge_is_independent_of_order_and_removal_preserves_other_service() {
        for reverse in [false, true] {
            let airplay = service(
                AIRPLAY,
                "192.0.2.10",
                &[
                    ("deviceid", "00:11:22:33:44:55"),
                    ("model", "AudioAccessory5,1"),
                    ("tsid", STEREO),
                    ("gpn", "Living room"),
                    ("srcvers", "980.2"),
                ],
            );
            let raop = service(
                RAOP,
                "192.0.2.11",
                &[
                    ("am", "AudioAccessory5,1"),
                    ("pw", "true"),
                    ("sf", "8"),
                    ("srcvers", "obsolete"),
                ],
            );
            let mut catalog = DiscoveryCatalog::default();
            let mut events = vec![(RAOP, raop.clone()), (AIRPLAY, airplay.clone())];
            if reverse {
                events.reverse();
            }
            for (kind, info) in events {
                catalog
                    .apply(kind, ServiceEvent::ServiceResolved(info))
                    .unwrap();
            }
            assert_eq!(catalog.devices.len(), 1);
            let device = &catalog.devices[0];
            assert_eq!(device.port, 7000);
            assert_eq!(device.addresses.len(), 2);
            assert!(device.requires_password && device.requires_pin);
            assert_eq!(device.txt["srcvers"], "980.2");
            assert_eq!(
                device.native_stereo().unwrap().id,
                STEREO.to_ascii_lowercase()
            );
            let after = catalog
                .apply(
                    AIRPLAY,
                    ServiceEvent::ServiceRemoved(
                        AIRPLAY.to_ascii_uppercase(),
                        airplay.get_fullname().to_ascii_uppercase(),
                    ),
                )
                .unwrap();
            assert_eq!(after.len(), 1);
            assert_eq!(after[0].port, 5000);
            assert!(after[0].native_stereo().is_none());
            let after = catalog
                .apply(
                    RAOP,
                    ServiceEvent::ServiceRemoved(RAOP.into(), raop.get_fullname().into()),
                )
                .unwrap();
            assert!(after.is_empty());
        }
    }

    #[test]
    fn advertisement_update_replaces_expired_addresses_and_group_metadata() {
        let mut catalog = DiscoveryCatalog::default();
        catalog.apply(
            AIRPLAY,
            ServiceEvent::ServiceResolved(service(
                AIRPLAY,
                "192.0.2.10",
                &[
                    ("deviceid", "00:11:22:33:44:55"),
                    ("model", "AudioAccessory5,1"),
                    ("tsid", STEREO),
                ],
            )),
        );
        let updated = service(
            AIRPLAY,
            "192.0.2.20",
            &[
                ("deviceid", "00:11:22:33:44:55"),
                ("model", "AudioAccessory5,1"),
            ],
        );
        let devices = catalog
            .apply(AIRPLAY, ServiceEvent::ServiceResolved(updated.clone()))
            .unwrap();
        assert_eq!(
            devices[0].addresses,
            vec!["192.0.2.20".parse::<IpAddr>().unwrap()]
        );
        assert!(devices[0].native_stereo().is_none());
        assert!(catalog
            .apply(AIRPLAY, ServiceEvent::ServiceResolved(updated))
            .is_none());
        assert!(catalog
            .apply(
                AIRPLAY,
                ServiceEvent::ServiceFound(AIRPLAY.into(), "unknown".into())
            )
            .is_none());
        assert!(catalog
            .apply(
                AIRPLAY,
                ServiceEvent::ServiceRemoved(AIRPLAY.into(), "unknown".into())
            )
            .is_none());
    }

    #[test]
    fn changed_physical_identity_does_not_leave_a_ghost_device() {
        let mut catalog = DiscoveryCatalog::default();
        for id in ["00:11:22:33:44:55", "AA:BB:CC:DD:EE:FF"] {
            let devices = catalog
                .apply(
                    AIRPLAY,
                    ServiceEvent::ServiceResolved(service(
                        AIRPLAY,
                        "192.0.2.10",
                        &[("deviceid", id)],
                    )),
                )
                .unwrap();
            assert_eq!(devices.len(), 1);
            assert_eq!(devices[0].id, id.replace(':', ""));
        }
    }

    #[test]
    fn native_stereo_requires_homepod_and_canonical_nonzero_tsid() {
        let mut device = from_service(&service(
            AIRPLAY,
            "192.0.2.10",
            &[
                ("deviceid", "00:11:22:33:44:55"),
                ("model", "AudioAccessory5,1"),
                ("gid", STEREO),
                ("gpn", "Stereo pair"),
                ("tsm", "1"),
                ("igl", "1"),
            ],
        ));
        assert!(
            device.native_stereo().is_none(),
            "name and playback-group fields are insufficient"
        );
        for invalid in [
            "",
            "00000000-0000-0000-0000-000000000000",
            "not-a-uuid",
            "EAFA36AA978554B2A537D9EE2A55CF1C",
            "EAFA36AA-9785-54B2-A537-D9EE2A55CF1Z",
            " EAFA36AA-9785-54B2-A537-D9EE2A55CF1C",
        ] {
            device.txt.insert("tsid".into(), invalid.into());
            assert!(
                device.native_stereo().is_none(),
                "invalid tsid accepted: {invalid}"
            );
        }
        device.txt.insert("tsid".into(), STEREO.into());
        assert_eq!(
            device.native_stereo().unwrap().name.as_deref(),
            Some("Stereo pair")
        );
        device.txt.insert("gpn".into(), "invalid\nname".into());
        assert!(device.native_stereo().unwrap().name.is_none());
        device.model = Some("AppleTV11,1".into());
        assert!(device.native_stereo().is_none());
    }
}
