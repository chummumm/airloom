use crate::{
    crypto::{self, ChannelCipher},
    protocol, AuthenticationKind, Error, Result,
};
use md5::Md5;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use zeroize::Zeroizing;

const MAX_HEADERS: usize = 32 * 1024;
const MAX_BODY: usize = 2 * 1024 * 1024;

pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Wire {
    stream: TcpStream,
    inbound: Vec<u8>,
    read_cipher: Option<ChannelCipher>,
    write_cipher: Option<ChannelCipher>,
}

impl Wire {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            inbound: Vec::new(),
            read_cipher: None,
            write_cipher: None,
        }
    }
    pub fn encrypted(stream: TcpStream, input_key: [u8; 32], output_key: [u8; 32]) -> Self {
        let mut wire = Self::new(stream);
        wire.enable_encryption(input_key, output_key);
        wire
    }
    pub fn enable_encryption(&mut self, input_key: [u8; 32], output_key: [u8; 32]) {
        self.read_cipher = Some(ChannelCipher::new(input_key));
        self.write_cipher = Some(ChannelCipher::new(output_key));
    }
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.stream.local_addr()?)
    }
    pub async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let encoded;
        let bytes = if let Some(cipher) = &mut self.write_cipher {
            encoded = cipher.encode(bytes)?;
            &encoded
        } else {
            bytes
        };
        self.stream.write_all(bytes).await?;
        Ok(())
    }
    async fn read_more(&mut self) -> Result<()> {
        if self.inbound.len() >= MAX_HEADERS + MAX_BODY {
            return Err(protocol("RTSP message exceeds size limit"));
        }
        if let Some(cipher) = &mut self.read_cipher {
            let mut length = [0; 2];
            self.stream.read_exact(&mut length).await?;
            let length_value = u16::from_le_bytes(length) as usize;
            if length_value > 1024 {
                return Err(protocol("oversized HAP frame"));
            }
            let mut ciphertext = vec![0; length_value + 16];
            self.stream.read_exact(&mut ciphertext).await?;
            self.inbound
                .extend(cipher.decode_frame(length, &ciphertext)?);
        } else {
            let mut buffer = [0; 4096];
            let count = self.stream.read(&mut buffer).await?;
            if count == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
            }
            self.inbound.extend_from_slice(&buffer[..count]);
        }
        Ok(())
    }
    pub async fn read_message(&mut self) -> Result<(String, BTreeMap<String, String>, Vec<u8>)> {
        let header_end = loop {
            if let Some(position) = self.inbound.windows(4).position(|w| w == b"\r\n\r\n") {
                break position + 4;
            }
            if self.inbound.len() > MAX_HEADERS {
                return Err(protocol("RTSP headers exceed limit"));
            }
            self.read_more().await?;
        };
        if header_end > MAX_HEADERS {
            return Err(protocol("RTSP headers exceed limit"));
        }
        let header = std::str::from_utf8(&self.inbound[..header_end])
            .map_err(|_| protocol("invalid RTSP header encoding"))?;
        let mut lines = header.split("\r\n");
        let first = lines
            .next()
            .ok_or_else(|| protocol("missing RTSP status line"))?
            .to_owned();
        let mut headers = BTreeMap::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (key, value) = line
                .split_once(':')
                .ok_or_else(|| protocol("malformed RTSP header"))?;
            let key = key.trim().to_ascii_lowercase();
            if headers.insert(key, value.trim().to_owned()).is_some() {
                return Err(protocol("duplicate RTSP header"));
            }
        }
        if headers.contains_key("transfer-encoding") {
            return Err(protocol("chunked RTSP bodies unsupported"));
        }
        let body_len: usize = headers
            .get("content-length")
            .map(|v| v.parse().map_err(|_| protocol("invalid Content-Length")))
            .transpose()?
            .unwrap_or(0);
        if body_len > MAX_BODY {
            return Err(protocol("RTSP body exceeds limit"));
        }
        while self.inbound.len() < header_end + body_len {
            self.read_more().await?;
        }
        let body = self.inbound[header_end..header_end + body_len].to_vec();
        self.inbound.drain(..header_end + body_len);
        Ok((first, headers, body))
    }
}

pub struct Connection {
    pub wire: Wire,
    cseq: u32,
    pub uri: String,
    pub session_uuid: String,
    pub identity: String,
    password: Option<Zeroizing<String>>,
    digest: Option<DigestChallenge>,
    session: Option<String>,
    poisoned: bool,
}

impl Connection {
    pub async fn connect(address: SocketAddr, password: Option<String>) -> Result<Self> {
        let stream = timeout(Duration::from_secs(6), TcpStream::connect(address))
            .await
            .map_err(|_| Error::Timeout("TCP connect"))??;
        stream.set_nodelay(true)?;
        let local = stream.local_addr()?.ip();
        let identity = hex::encode_upper(crypto::random::<8>());
        let session_uuid = format!(
            "{}-{}-{}-{}-{}",
            &identity[..8],
            &identity[8..12],
            &identity[12..],
            hex::encode(crypto::random::<2>()),
            hex::encode(crypto::random::<6>())
        );
        let host = if local.is_ipv6() {
            format!("[{local}]")
        } else {
            local.to_string()
        };
        Ok(Self {
            wire: Wire::new(stream),
            cseq: 0,
            uri: format!("rtsp://{host}/{}", u64::from_le_bytes(crypto::random())),
            session_uuid,
            identity,
            password: password.map(Zeroizing::new),
            digest: None,
            session: None,
            poisoned: false,
        })
    }
    pub fn secure(&mut self, secret: &[u8]) {
        self.wire.enable_encryption(
            crypto::derive(secret, b"Control-Salt", b"Control-Read-Encryption-Key"),
            crypto::derive(secret, b"Control-Salt", b"Control-Write-Encryption-Key"),
        );
    }
    pub async fn exchange(
        &mut self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
        extra: &[(&str, &str)],
    ) -> Result<Response> {
        if self.poisoned {
            return Err(Error::SessionLost(
                "previous control request did not complete".into(),
            ));
        }
        self.poisoned = true;
        let result = timeout(
            Duration::from_secs(6),
            self.exchange_inner(method, path, content_type, body, extra),
        )
        .await;
        match result {
            Ok(Ok(response)) => {
                self.poisoned = false;
                Ok(response)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(Error::Timeout("RTSP response")),
        }
    }
    async fn exchange_inner(
        &mut self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
        extra: &[(&str, &str)],
    ) -> Result<Response> {
        let mut response = self.once(method, path, content_type, body, extra).await?;
        if response.status == 401 {
            let challenge = response.headers.get("www-authenticate");
            if let (Some(challenge), Some(_)) = (challenge, &self.password) {
                self.digest = Some(DigestChallenge::parse(challenge)?);
                response = self.once(method, path, content_type, body, extra).await?;
            }
        }
        Ok(response)
    }
    async fn once(
        &mut self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
        extra: &[(&str, &str)],
    ) -> Result<Response> {
        self.cseq = self
            .cseq
            .checked_add(1)
            .ok_or_else(|| protocol("RTSP sequence exhausted"))?;
        let protocol_name = if path.starts_with("/pair-") {
            "HTTP/1.1"
        } else {
            "RTSP/1.0"
        };
        let mut request = format!("{method} {path} {protocol_name}\r\nCSeq: {}\r\nUser-Agent: AirPlay/550.10\r\nDACP-ID: {}\r\nX-Apple-ProtocolVersion: 1\r\nX-Apple-Session-ID: {}\r\nContent-Length: {}\r\n",self.cseq,self.identity,self.session_uuid,body.len());
        if !content_type.is_empty() {
            request.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        if let Some(session) = &self.session {
            request.push_str(&format!("Session: {session}\r\n"));
        }
        if let (Some(challenge), Some(password)) = (&mut self.digest, &self.password) {
            request.push_str(&format!(
                "Authorization: {}\r\n",
                challenge.authorization(method, path, password)?
            ));
        }
        for (key, value) in extra {
            if key.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                return Err(protocol("invalid request header"));
            }
            request.push_str(&format!("{key}: {value}\r\n"));
        }
        request.push_str("\r\n");
        let mut bytes = request.into_bytes();
        bytes.extend_from_slice(body);
        self.wire.write(&bytes).await?;
        let (status_line, headers, body) = self.wire.read_message().await?;
        if !status_line.starts_with("RTSP/") && !status_line.starts_with("HTTP/") {
            return Err(protocol("unexpected control-channel request"));
        }
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| protocol("invalid RTSP status"))?;
        if let Some(sequence) = headers.get("cseq") {
            if sequence.parse::<u32>().ok() != Some(self.cseq) {
                return Err(protocol("RTSP response sequence mismatch"));
            }
        }
        if let Some(session) = headers.get("session") {
            self.session = Some(session.split(';').next().unwrap_or(session).to_owned());
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
    pub fn check(&self, response: &Response, stage: &'static str) -> Result<()> {
        match response.status {
            200..=299 => Ok(()),
            401 => Err(if self.password.is_some() {
                Error::AuthenticationRejected {
                    kind: AuthenticationKind::Password,
                }
            } else {
                Error::AuthenticationRequired {
                    kind: AuthenticationKind::Password,
                }
            }),
            403 => Err(Error::AccessDenied { stage, status: 403 }),
            470 => Err(Error::AuthenticationRequired {
                kind: AuthenticationKind::Pin,
            }),
            status => Err(Error::Receiver { stage, status }),
        }
    }
}

struct DigestChallenge {
    realm: String,
    nonce: String,
    opaque: Option<String>,
    qop: bool,
    sha256: bool,
    count: u32,
}
impl DigestChallenge {
    fn parse(value: &str) -> Result<Self> {
        let payload = value
            .strip_prefix("Digest ")
            .or_else(|| value.strip_prefix("digest "))
            .ok_or_else(|| {
                Error::Unsupported("receiver authentication challenge is not Digest".into())
            })?;
        let mut fields = BTreeMap::new();
        let mut start = 0;
        let mut quoted = false;
        for (i, c) in payload
            .char_indices()
            .chain(std::iter::once((payload.len(), ',')))
        {
            if c == '"' {
                quoted = !quoted;
            }
            if c == ',' && !quoted {
                let part = payload[start..i].trim();
                let (key, value) = part
                    .split_once('=')
                    .ok_or_else(|| protocol("invalid Digest challenge"))?;
                let value = value.trim().trim_matches('"');
                if value.contains(['\r', '\n', '"', '\\']) {
                    return Err(protocol("unsupported Digest quoted escape"));
                }
                fields.insert(key.trim().to_ascii_lowercase(), value.to_owned());
                start = i + 1;
            }
        }
        if quoted {
            return Err(protocol("unterminated Digest quote"));
        }
        let realm = fields
            .remove("realm")
            .ok_or_else(|| protocol("Digest realm missing"))?;
        let nonce = fields
            .remove("nonce")
            .ok_or_else(|| protocol("Digest nonce missing"))?;
        let sha256 = match fields
            .get("algorithm")
            .map(|v| v.to_ascii_uppercase())
            .as_deref()
        {
            None | Some("MD5") => false,
            Some("SHA-256") => true,
            _ => return Err(Error::Unsupported("Digest algorithm".into())),
        };
        let qop = match fields.get("qop") {
            None => false,
            Some(q) if q.split(',').any(|s| s.trim() == "auth") => true,
            _ => return Err(Error::Unsupported("Digest qop".into())),
        };
        Ok(Self {
            realm,
            nonce,
            opaque: fields.remove("opaque"),
            qop,
            sha256,
            count: 0,
        })
    }
    fn authorization(&mut self, method: &str, uri: &str, password: &str) -> Result<String> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| protocol("Digest counter exhausted"))?;
        let hash = |s: String| {
            if self.sha256 {
                hex::encode(Sha256::digest(s.as_bytes()))
            } else {
                hex::encode(Md5::digest(s.as_bytes()))
            }
        };
        let username = if self.realm.eq_ignore_ascii_case("raop") {
            "iTunes"
        } else {
            "AirPlay"
        };
        let a1 = hash(format!("{username}:{}:{password}", self.realm));
        let a2 = hash(format!("{method}:{uri}"));
        let cnonce = hex::encode(crypto::random::<16>());
        let nc = format!("{:08x}", self.count);
        let response = if self.qop {
            hash(format!("{a1}:{}:{nc}:{cnonce}:auth:{a2}", self.nonce))
        } else {
            hash(format!("{a1}:{}:{a2}", self.nonce))
        };
        let mut value = format!("Digest username=\"{username}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", response=\"{response}\", algorithm={}",self.realm,self.nonce,if self.sha256{"SHA-256"}else{"MD5"});
        if self.qop {
            value.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""));
        }
        if let Some(opaque) = &self.opaque {
            value.push_str(&format!(", opaque=\"{opaque}\""));
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn digest_challenge_accepts_qop_lists_but_rejects_unknown_algorithms() {
        assert!(
            DigestChallenge::parse("Digest realm=\"raop\", nonce=\"n\", qop=\"auth,auth-int\"")
                .unwrap()
                .qop
        );
        assert!(
            DigestChallenge::parse("Digest realm=\"r\", nonce=\"n\", algorithm=unknown").is_err()
        );
        assert!(DigestChallenge::parse("Basic realm=\"r\"").is_err());
    }
}
