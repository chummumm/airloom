use crate::{Error, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::IpAddr,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

pub async fn discover(duration: Duration) -> Result<Vec<Device>> {
    if duration > Duration::from_secs(30) {
        return Err(Error::InvalidArgument(
            "discovery window must be at most 30 seconds".into(),
        ));
    }
    tokio::task::spawn_blocking(move || {
        let daemon = ServiceDaemon::new().map_err(|e| Error::Discovery(e.to_string()))?;
        let scan = || -> Result<Vec<Device>> {
            let airplay = daemon
                .browse("_airplay._tcp.local.")
                .map_err(|e| Error::Discovery(e.to_string()))?;
            let raop = daemon
                .browse("_raop._tcp.local.")
                .map_err(|e| Error::Discovery(e.to_string()))?;
            let deadline = Instant::now() + duration;
            let mut entries = BTreeMap::<String, (Device, bool)>::new();
            while Instant::now() < deadline {
                for (receiver, preferred) in [(&airplay, true), (&raop, false)] {
                    while let Ok(event) = receiver.try_recv() {
                        if let ServiceEvent::ServiceResolved(info) = event {
                            let device = from_service(&info);
                            match entries.get_mut(&device.id) {
                                Some((current, is_preferred)) => {
                                    let mut addresses = current.addresses.clone();
                                    addresses.extend(device.addresses.iter().copied());
                                    addresses.sort();
                                    addresses.dedup();
                                    let mut txt = current.txt.clone();
                                    txt.extend(device.txt.clone());
                                    if preferred || !*is_preferred {
                                        *current = device;
                                        *is_preferred = preferred;
                                    }
                                    current.addresses = addresses;
                                    current.txt = txt;
                                    current.requires_password |= current
                                        .txt
                                        .get("pw")
                                        .is_some_and(|s| s.eq_ignore_ascii_case("true"));
                                }
                                None => {
                                    entries.insert(device.id.clone(), (device, preferred));
                                }
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(entries.into_values().map(|(device, _)| device).collect())
        };
        let result = scan();
        let _ = daemon.stop_browse("_airplay._tcp.local.");
        let _ = daemon.stop_browse("_raop._tcp.local.");
        let _ = daemon.shutdown();
        result
    })
    .await
    .map_err(|e| Error::Discovery(e.to_string()))?
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
    #[test]
    fn apple_features_are_low_word_first() {
        assert_eq!(
            feature_bits("0x12345678,0x01234567"),
            Some(0x0123456712345678)
        );
        assert_eq!(feature_bits("0x100000000,1"), None);
        assert_eq!(feature_bits("0x12,wrong"), None);
    }
}
