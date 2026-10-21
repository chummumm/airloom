use crate::{
    crypto::{self, SrpExchange},
    protocol,
    rtsp::Connection,
    tlv, AuthenticationKind, Device, Error, Result,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, net::SocketAddr};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Serialize, Deserialize)]
pub struct PairingCredentials {
    version: u8,
    device_id: String,
    controller_id: Vec<u8>,
    controller_seed: [u8; 32],
    receiver_id: Vec<u8>,
    receiver_key: [u8; 32],
}
impl fmt::Debug for PairingCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCredentials([redacted])")
    }
}
impl Drop for PairingCredentials {
    fn drop(&mut self) {
        self.controller_seed.zeroize();
    }
}

pub struct PairingSession {
    connection: Connection,
    device_id: String,
    salt: Vec<u8>,
    server_public: Vec<u8>,
}

pub(crate) fn address(device: &Device) -> Result<SocketAddr> {
    let ip = device
        .addresses
        .iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| device.addresses.first())
        .ok_or_else(|| Error::InvalidArgument("receiver has no reachable address".into()))?;
    if device.port == 0 {
        return Err(Error::InvalidArgument(
            "receiver port must be nonzero".into(),
        ));
    }
    Ok(SocketAddr::new(*ip, device.port))
}

fn unpack(body: &[u8], state: u8, kind: AuthenticationKind) -> Result<BTreeMap<u8, Vec<u8>>> {
    let fields = tlv::decode(body)?;
    if fields.contains_key(&tlv::ERROR) {
        return Err(Error::AuthenticationRejected { kind });
    }
    if tlv::required(&fields, tlv::STATE)? != [state] {
        return Err(protocol("unexpected pairing state"));
    }
    Ok(fields)
}

async fn post(
    connection: &mut Connection,
    path: &str,
    body: &[u8],
    transient: bool,
    state: u8,
    kind: AuthenticationKind,
) -> Result<BTreeMap<u8, Vec<u8>>> {
    let response = connection
        .exchange(
            "POST",
            path,
            "application/octet-stream",
            body,
            &[("X-Apple-HKP", if transient { "4" } else { "3" })],
        )
        .await?;
    if response.status == 401 {
        return Err(Error::AuthenticationRejected { kind });
    }
    if response.status == 470 {
        return Err(Error::AuthenticationRequired {
            kind: AuthenticationKind::Pin,
        });
    }
    connection.check(&response, "pairing")?;
    unpack(&response.body, state, kind)
}

pub(crate) async fn transient(
    connection: &mut Connection,
    password: Option<&str>,
) -> Result<Zeroizing<Vec<u8>>> {
    let kind = AuthenticationKind::Password;
    let challenge = post(
        connection,
        "/pair-setup",
        &tlv::encode(&[
            (tlv::METHOD, &[0]),
            (tlv::STATE, &[1]),
            (tlv::FLAGS, &[0x10]),
        ]),
        true,
        2,
        kind,
    )
    .await?;
    let salt = tlv::required(&challenge, tlv::SALT)?;
    let server = tlv::required(&challenge, tlv::PUBLIC_KEY)?;
    let secret = Zeroizing::new(password.unwrap_or("3939").as_bytes().to_vec());
    let srp = SrpExchange::begin(&secret, salt, server)?;
    let response = post(
        connection,
        "/pair-setup",
        &tlv::encode(&[
            (tlv::STATE, &[3]),
            (tlv::PUBLIC_KEY, &srp.public),
            (tlv::PROOF, &srp.proof),
        ]),
        true,
        4,
        kind,
    )
    .await?;
    srp.verify(tlv::required(&response, tlv::PROOF)?)
}

impl PairingSession {
    /// Starts on-screen PIN pairing and retains the same TCP exchange until finish.
    pub async fn start(device: &Device) -> Result<Self> {
        let mut connection = Connection::connect(address(device)?, None).await?;
        let start = connection
            .exchange("POST", "/pair-pin-start", "", &[], &[("X-Apple-HKP", "3")])
            .await?;
        connection.check(&start, "start PIN pairing")?;
        let challenge = post(
            &mut connection,
            "/pair-setup",
            &tlv::encode(&[(tlv::METHOD, &[0]), (tlv::STATE, &[1])]),
            false,
            2,
            AuthenticationKind::Pin,
        )
        .await?;
        Ok(Self {
            connection,
            device_id: device.id.clone(),
            salt: tlv::required(&challenge, tlv::SALT)?.to_vec(),
            server_public: tlv::required(&challenge, tlv::PUBLIC_KEY)?.to_vec(),
        })
    }
    pub async fn finish(mut self, pin: &str) -> Result<PairingCredentials> {
        if pin.is_empty() || pin.len() > 64 {
            return Err(Error::InvalidArgument("PIN is empty or too long".into()));
        }
        let kind = AuthenticationKind::Pin;
        let srp = SrpExchange::begin(pin.as_bytes(), &self.salt, &self.server_public)?;
        let response = post(
            &mut self.connection,
            "/pair-setup",
            &tlv::encode(&[
                (tlv::STATE, &[3]),
                (tlv::PUBLIC_KEY, &srp.public),
                (tlv::PROOF, &srp.proof),
            ]),
            false,
            4,
            kind,
        )
        .await?;
        let shared =
            srp.verify(tlv::required(&response, tlv::PROOF)?)
                .map_err(|error| match error {
                    Error::AuthenticationRejected { .. } => Error::AuthenticationRejected { kind },
                    other => other,
                })?;
        let encryption_key = Zeroizing::new(crypto::derive(
            &shared,
            b"Pair-Setup-Encrypt-Salt",
            b"Pair-Setup-Encrypt-Info",
        ));
        let controller_seed = crypto::random::<32>();
        let signing = SigningKey::from_bytes(&controller_seed);
        let controller_id = hex::encode_upper(crypto::random::<16>()).into_bytes();
        let controller_public = signing.verifying_key().to_bytes();
        let mut info = crypto::derive(
            &shared,
            b"Pair-Setup-Controller-Sign-Salt",
            b"Pair-Setup-Controller-Sign-Info",
        )
        .to_vec();
        info.extend_from_slice(&controller_id);
        info.extend_from_slice(&controller_public);
        let signature = signing.sign(&info).to_bytes();
        let encrypted = crypto::seal(
            &encryption_key,
            &crypto::named_nonce(b"PS-Msg05"),
            &[],
            &tlv::encode(&[
                (tlv::IDENTIFIER, &controller_id),
                (tlv::PUBLIC_KEY, &controller_public),
                (tlv::SIGNATURE, &signature),
            ]),
        )?;
        let finish = post(
            &mut self.connection,
            "/pair-setup",
            &tlv::encode(&[(tlv::STATE, &[5]), (tlv::ENCRYPTED, &encrypted)]),
            false,
            6,
            kind,
        )
        .await?;
        let plain = crypto::open(
            &encryption_key,
            &crypto::named_nonce(b"PS-Msg06"),
            &[],
            tlv::required(&finish, tlv::ENCRYPTED)?,
        )?;
        let fields = tlv::decode(&plain)?;
        let receiver_id = tlv::required(&fields, tlv::IDENTIFIER)?.to_vec();
        if receiver_id.is_empty() || receiver_id.len() > 256 {
            return Err(protocol("invalid receiver pairing identifier"));
        }
        let receiver_key: [u8; 32] = tlv::required(&fields, tlv::PUBLIC_KEY)?
            .try_into()
            .map_err(|_| protocol("invalid receiver pairing key"))?;
        let signature = Signature::from_slice(tlv::required(&fields, tlv::SIGNATURE)?)
            .map_err(|_| protocol("invalid pairing signature length"))?;
        let mut info = crypto::derive(
            &shared,
            b"Pair-Setup-Accessory-Sign-Salt",
            b"Pair-Setup-Accessory-Sign-Info",
        )
        .to_vec();
        info.extend_from_slice(&receiver_id);
        info.extend_from_slice(&receiver_key);
        VerifyingKey::from_bytes(&receiver_key)
            .map_err(|_| protocol("invalid receiver signing key"))?
            .verify_strict(&info, &signature)
            .map_err(|_| Error::AuthenticationRejected { kind })?;
        Ok(PairingCredentials {
            version: 1,
            device_id: self.device_id,
            controller_id,
            controller_seed,
            receiver_id,
            receiver_key,
        })
    }
}

/// Convenience for devices with a static PIN. For displayed PINs use
/// PairingSession::start, ask for the displayed PIN, then finish on that session.
pub async fn pair(device: &Device, pin: &str) -> Result<PairingCredentials> {
    PairingSession::start(device).await?.finish(pin).await
}

pub(crate) async fn verify(
    connection: &mut Connection,
    device: &Device,
    credentials: &PairingCredentials,
) -> Result<Zeroizing<Vec<u8>>> {
    let kind = AuthenticationKind::Credentials;
    if credentials.version != 1
        || credentials.device_id != device.id
        || credentials.controller_id.is_empty()
        || credentials.controller_id.len() > 256
        || credentials.receiver_id.is_empty()
        || credentials.receiver_id.len() > 256
    {
        return Err(Error::AuthenticationRejected { kind });
    }
    let private = StaticSecret::from(crypto::random::<32>());
    let public = PublicKey::from(&private).to_bytes();
    let response = post(
        connection,
        "/pair-verify",
        &tlv::encode(&[(tlv::STATE, &[1]), (tlv::PUBLIC_KEY, &public)]),
        false,
        2,
        kind,
    )
    .await?;
    let remote: [u8; 32] = tlv::required(&response, tlv::PUBLIC_KEY)?
        .try_into()
        .map_err(|_| protocol("invalid pair-verify public key"))?;
    let shared = private.diffie_hellman(&PublicKey::from(remote));
    if !shared.was_contributory() {
        return Err(Error::AuthenticationRejected { kind });
    }
    let key = Zeroizing::new(crypto::derive(
        shared.as_bytes(),
        b"Pair-Verify-Encrypt-Salt",
        b"Pair-Verify-Encrypt-Info",
    ));
    let plaintext = crypto::open(
        &key,
        &crypto::named_nonce(b"PV-Msg02"),
        &[],
        tlv::required(&response, tlv::ENCRYPTED)?,
    )?;
    let fields = tlv::decode(&plaintext)?;
    if tlv::required(&fields, tlv::IDENTIFIER)? != credentials.receiver_id {
        return Err(Error::AuthenticationRejected { kind });
    }
    let signature = Signature::from_slice(tlv::required(&fields, tlv::SIGNATURE)?)
        .map_err(|_| protocol("invalid pair-verify signature length"))?;
    let mut message = remote.to_vec();
    message.extend_from_slice(&credentials.receiver_id);
    message.extend_from_slice(&public);
    VerifyingKey::from_bytes(&credentials.receiver_key)
        .map_err(|_| protocol("invalid saved receiver key"))?
        .verify_strict(&message, &signature)
        .map_err(|_| Error::AuthenticationRejected { kind })?;
    let mut message = public.to_vec();
    message.extend_from_slice(&credentials.controller_id);
    message.extend_from_slice(&remote);
    let signature = SigningKey::from_bytes(&credentials.controller_seed)
        .sign(&message)
        .to_bytes();
    let encrypted = crypto::seal(
        &key,
        &crypto::named_nonce(b"PV-Msg03"),
        &[],
        &tlv::encode(&[
            (tlv::IDENTIFIER, &credentials.controller_id),
            (tlv::SIGNATURE, &signature),
        ]),
    )?;
    post(
        connection,
        "/pair-verify",
        &tlv::encode(&[(tlv::STATE, &[3]), (tlv::ENCRYPTED, &encrypted)]),
        false,
        4,
        kind,
    )
    .await?;
    Ok(Zeroizing::new(shared.as_bytes().to_vec()))
}
