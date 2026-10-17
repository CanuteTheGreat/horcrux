//! WebAuthn (FIDO2 security keys, passkeys, platform authenticators) as a
//! second factor.
//!
//! Relying-party side of the W3C WebAuthn Level 2 ceremonies, for the two
//! signature algorithms every authenticator offers: ES256 (COSE -7) and
//! RS256 (COSE -257). Attestation is requested as `none` and not evaluated:
//! we trust the key the user registers while logged in, not its make/model.
//! Checked on every ceremony: challenge, origin, ceremony type, RP ID hash,
//! user-presence flag, signature, and (for assertions) a non-decreasing
//! signature counter to detect cloned authenticators.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ciborium::value::Value as Cbor;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

pub const COSE_ES256: i64 = -7;
pub const COSE_RS256: i64 = -257;
const FLAG_UP: u8 = 0x01;
const FLAG_AT: u8 = 0x40;

/// A registered credential, stored as JSON in `auth_tfa.data`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredCredential {
    /// base64url credential id
    pub credential_id: String,
    pub alg: i64,
    /// ES256: base64url x and y. RS256: base64url n and e.
    pub key_a: String,
    pub key_b: String,
    pub rp_id: String,
}

/// Where the ceremony happens: the RP ID (host name) and accepted origins.
#[derive(Debug, Clone)]
pub struct RelyingParty {
    pub id: String,
    pub origins: Vec<String>,
}

impl RelyingParty {
    /// Derive from the request's `Host` header (`host[:port]`): RP ID is the
    /// host name, origin `https://host[:port]`. `HORCRUX_WEBAUTHN_ORIGIN`
    /// (comma-separated) overrides the origin list, e.g. behind a proxy.
    pub fn from_host(host: &str) -> Self {
        let host = host.trim();
        let id = if host.starts_with('[') {
            host.split(']')
                .next()
                .unwrap_or(host)
                .trim_start_matches('[')
                .to_string()
        } else {
            host.split(':').next().unwrap_or(host).to_string()
        };
        let origins = match std::env::var("HORCRUX_WEBAUTHN_ORIGIN") {
            Ok(v) if !v.trim().is_empty() => v.split(',').map(|s| s.trim().to_string()).collect(),
            _ => vec![format!("https://{}", host)],
        };
        let id = std::env::var("HORCRUX_WEBAUTHN_RP_ID").unwrap_or(id);
        Self { id, origins }
    }
}

pub fn new_challenge() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    B64.encode(b)
}

/// `PublicKeyCredentialCreationOptions` (JSON, binary fields base64url).
pub fn creation_options(
    rp: &RelyingParty,
    user_id: &str,
    username: &str,
    challenge: &str,
    exclude: &[StoredCredential],
) -> serde_json::Value {
    json!({
        "challenge": challenge,
        "rp": { "id": rp.id, "name": "Horcrux" },
        "user": { "id": B64.encode(user_id.as_bytes()), "name": username, "displayName": username },
        "pubKeyCredParams": [
            { "type": "public-key", "alg": COSE_ES256 },
            { "type": "public-key", "alg": COSE_RS256 }
        ],
        "timeout": 120000,
        "attestation": "none",
        "authenticatorSelection": { "userVerification": "preferred", "residentKey": "discouraged" },
        "excludeCredentials": exclude.iter().map(|c| json!({"type": "public-key", "id": c.credential_id})).collect::<Vec<_>>()
    })
}

/// `PublicKeyCredentialRequestOptions`.
pub fn request_options(
    rp: &RelyingParty,
    challenge: &str,
    allow: &[StoredCredential],
) -> serde_json::Value {
    json!({
        "challenge": challenge,
        "rpId": rp.id,
        "timeout": 120000,
        "userVerification": "preferred",
        "allowCredentials": allow.iter().map(|c| json!({"type": "public-key", "id": c.credential_id})).collect::<Vec<_>>()
    })
}

/// Browser `PublicKeyCredential` serialized with base64url binaries.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CredentialResponse {
    pub id: String,
    #[serde(default, rename = "rawId")]
    pub raw_id: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    pub response: AuthenticatorResponse,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthenticatorResponse {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    #[serde(default, rename = "attestationObject")]
    pub attestation_object: Option<String>,
    #[serde(default, rename = "authenticatorData")]
    pub authenticator_data: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default, rename = "userHandle")]
    pub user_handle: Option<String>,
}

fn b64(s: &str) -> Result<Vec<u8>, String> {
    let trimmed = s.trim_end_matches('=');
    B64.decode(trimmed)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(trimmed))
        .map_err(|e| format!("bad base64: {}", e))
}

fn check_client_data(
    raw: &[u8],
    ceremony: &str,
    challenge: &str,
    rp: &RelyingParty,
) -> Result<(), String> {
    let cd: serde_json::Value =
        serde_json::from_slice(raw).map_err(|e| format!("clientDataJSON: {}", e))?;
    if cd["type"] != ceremony {
        return Err(format!("wrong ceremony type {:?}", cd["type"]));
    }
    let got = cd["challenge"].as_str().unwrap_or("");
    if b64(got)? != b64(challenge)? {
        return Err("challenge mismatch".into());
    }
    let origin = cd["origin"].as_str().unwrap_or("");
    if !rp.origins.iter().any(|o| o == origin) {
        return Err(format!(
            "origin {} not accepted (expected {:?})",
            origin, rp.origins
        ));
    }
    Ok(())
}

struct AuthData {
    flags: u8,
    sign_count: u32,
    credential: Option<(Vec<u8>, Vec<u8>)>, // (cred id, COSE key bytes)
}

fn parse_auth_data(data: &[u8], rp: &RelyingParty) -> Result<AuthData, String> {
    if data.len() < 37 {
        return Err("authenticator data too short".into());
    }
    let rp_hash = Sha256::digest(rp.id.as_bytes());
    if data[..32] != rp_hash[..] {
        return Err("RP ID hash mismatch".into());
    }
    let flags = data[32];
    if flags & FLAG_UP == 0 {
        return Err("user presence flag not set".into());
    }
    let sign_count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
    let mut credential = None;
    if flags & FLAG_AT != 0 {
        if data.len() < 55 {
            return Err("attested credential data truncated".into());
        }
        let len = u16::from_be_bytes([data[53], data[54]]) as usize;
        let start = 55;
        if data.len() < start + len {
            return Err("credential id truncated".into());
        }
        let cred_id = data[start..start + len].to_vec();
        // the COSE key is one CBOR item; extensions may follow it
        let rest = &data[start + len..];
        let mut cursor = std::io::Cursor::new(rest);
        let _: Cbor =
            ciborium::de::from_reader(&mut cursor).map_err(|e| format!("COSE key: {}", e))?;
        let used = cursor.position() as usize;
        credential = Some((cred_id, rest[..used].to_vec()));
    }
    Ok(AuthData {
        flags,
        sign_count,
        credential,
    })
}

fn cose_get(map: &[(Cbor, Cbor)], key: i64) -> Option<&Cbor> {
    map.iter().find_map(|(k, v)| match k {
        Cbor::Integer(i) if i128::from(*i) == key as i128 => Some(v),
        _ => None,
    })
}

fn cose_int(map: &[(Cbor, Cbor)], key: i64) -> Option<i64> {
    match cose_get(map, key)? {
        Cbor::Integer(i) => i64::try_from(i128::from(*i)).ok(),
        _ => None,
    }
}

fn cose_bytes(map: &[(Cbor, Cbor)], key: i64) -> Option<Vec<u8>> {
    match cose_get(map, key)? {
        Cbor::Bytes(b) => Some(b.clone()),
        _ => None,
    }
}

fn parse_cose_key(bytes: &[u8]) -> Result<(i64, Vec<u8>, Vec<u8>), String> {
    let v: Cbor = ciborium::de::from_reader(bytes).map_err(|e| format!("COSE key: {}", e))?;
    let Cbor::Map(m) = v else {
        return Err("COSE key is not a map".into());
    };
    let alg = cose_int(&m, 3).ok_or("COSE key without alg")?;
    match alg {
        COSE_ES256 => {
            if cose_int(&m, 1) != Some(2) || cose_int(&m, -1) != Some(1) {
                return Err("ES256 key is not EC2/P-256".into());
            }
            let x = cose_bytes(&m, -2).ok_or("EC key without x")?;
            let y = cose_bytes(&m, -3).ok_or("EC key without y")?;
            if x.len() != 32 || y.len() != 32 {
                return Err("bad P-256 coordinates".into());
            }
            Ok((alg, x, y))
        }
        COSE_RS256 => {
            if cose_int(&m, 1) != Some(3) {
                return Err("RS256 key is not RSA".into());
            }
            let n = cose_bytes(&m, -1).ok_or("RSA key without n")?;
            let e = cose_bytes(&m, -2).ok_or("RSA key without e")?;
            if n.len() < 256 {
                return Err("RSA key shorter than 2048 bits".into());
            }
            Ok((alg, n, e))
        }
        other => Err(format!("unsupported COSE algorithm {}", other)),
    }
}

/// Verify a registration (`navigator.credentials.create`) response.
pub fn verify_registration(
    resp: &CredentialResponse,
    challenge: &str,
    rp: &RelyingParty,
) -> Result<StoredCredential, String> {
    let cd = b64(&resp.response.client_data_json)?;
    check_client_data(&cd, "webauthn.create", challenge, rp)?;
    let att = b64(resp
        .response
        .attestation_object
        .as_deref()
        .ok_or("missing attestationObject")?)?;
    let att: Cbor =
        ciborium::de::from_reader(&att[..]).map_err(|e| format!("attestationObject: {}", e))?;
    let Cbor::Map(att) = att else {
        return Err("attestationObject is not a map".into());
    };
    let auth_data = att
        .iter()
        .find_map(|(k, v)| match (k, v) {
            (Cbor::Text(t), Cbor::Bytes(b)) if t == "authData" => Some(b.clone()),
            _ => None,
        })
        .ok_or("attestationObject without authData")?;
    let ad = parse_auth_data(&auth_data, rp)?;
    let (cred_id, cose) = ad.credential.ok_or("no attested credential data")?;
    if B64.encode(&cred_id) != resp.id.trim_end_matches('=') {
        return Err("credential id mismatch".into());
    }
    let (alg, a, b) = parse_cose_key(&cose)?;
    Ok(StoredCredential {
        credential_id: B64.encode(&cred_id),
        alg,
        key_a: B64.encode(a),
        key_b: B64.encode(b),
        rp_id: rp.id.clone(),
    })
}

/// Verify an assertion (`navigator.credentials.get`) against the user's
/// credentials. Returns the index of the credential used and its new counter.
pub fn verify_assertion(
    resp: &CredentialResponse,
    challenge: &str,
    rp: &RelyingParty,
    creds: &[(StoredCredential, i64)],
) -> Result<(usize, u32), String> {
    let id = resp.id.trim_end_matches('=');
    let (idx, (cred, stored_count)) = creds
        .iter()
        .enumerate()
        .find(|(_, (c, _))| c.credential_id == id)
        .ok_or("unknown credential")?;
    let cd = b64(&resp.response.client_data_json)?;
    check_client_data(&cd, "webauthn.get", challenge, rp)?;
    let auth = b64(resp
        .response
        .authenticator_data
        .as_deref()
        .ok_or("missing authenticatorData")?)?;
    let ad = parse_auth_data(&auth, rp)?;
    let sig = b64(resp
        .response
        .signature
        .as_deref()
        .ok_or("missing signature")?)?;
    let mut signed = auth.clone();
    signed.extend_from_slice(&Sha256::digest(&cd));
    verify_signature(cred, &signed, &sig)?;
    let _ = ad.flags;
    if (ad.sign_count != 0 || *stored_count != 0) && i64::from(ad.sign_count) <= *stored_count {
        return Err("signature counter did not increase (possible cloned authenticator)".into());
    }
    Ok((idx, ad.sign_count))
}

fn verify_signature(cred: &StoredCredential, msg: &[u8], sig: &[u8]) -> Result<(), String> {
    match cred.alg {
        COSE_ES256 => {
            use p256::ecdsa::signature::Verifier;
            let mut point = vec![0x04];
            point.extend(b64(&cred.key_a)?);
            point.extend(b64(&cred.key_b)?);
            let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(&point)
                .map_err(|e| format!("stored key: {}", e))?;
            let sig =
                p256::ecdsa::Signature::from_der(sig).map_err(|e| format!("signature: {}", e))?;
            key.verify(msg, &sig)
                .map_err(|_| "bad signature".to_string())
        }
        COSE_RS256 => {
            use rsa::pkcs1v15::{Signature, VerifyingKey};
            use rsa::signature::Verifier;
            let n = rsa::BigUint::from_bytes_be(&b64(&cred.key_a)?);
            let e = rsa::BigUint::from_bytes_be(&b64(&cred.key_b)?);
            let key = rsa::RsaPublicKey::new(n, e).map_err(|e| format!("stored key: {}", e))?;
            let vk = VerifyingKey::<Sha256>::new(key);
            let sig = Signature::try_from(sig).map_err(|e| format!("signature: {}", e))?;
            vk.verify(msg, &sig)
                .map_err(|_| "bad signature".to_string())
        }
        other => Err(format!("unsupported algorithm {}", other)),
    }
}

/// A software authenticator (ES256) used by the test suites (unit and live
/// end-to-end): produces exactly what a browser + security key would send.
pub mod soft {
    use super::*;
    use p256::ecdsa::{signature::Signer, Signature, SigningKey};

    pub struct SoftAuthenticator {
        pub key: SigningKey,
        pub cred_id: Vec<u8>,
        pub counter: u32,
    }

    impl SoftAuthenticator {
        pub fn new() -> Self {
            let mut id = vec![0u8; 16];
            rand::thread_rng().fill_bytes(&mut id);
            Self {
                key: SigningKey::random(&mut rand::thread_rng()),
                cred_id: id,
                counter: 0,
            }
        }

        fn client_data(ceremony: &str, challenge: &str, origin: &str) -> Vec<u8> {
            json!({"type": ceremony, "challenge": challenge, "origin": origin, "crossOrigin": false})
                .to_string()
                .into_bytes()
        }

        pub fn register(
            &mut self,
            options: &serde_json::Value,
            origin: &str,
        ) -> CredentialResponse {
            let challenge = options["challenge"].as_str().unwrap();
            let rp_id = options["rp"]["id"].as_str().unwrap();
            let point = self.key.verifying_key().to_encoded_point(false);
            let cose = Cbor::Map(vec![
                (Cbor::Integer(1.into()), Cbor::Integer(2.into())),
                (Cbor::Integer(3.into()), Cbor::Integer((-7).into())),
                (Cbor::Integer((-1).into()), Cbor::Integer(1.into())),
                (
                    Cbor::Integer((-2).into()),
                    Cbor::Bytes(point.x().unwrap().to_vec()),
                ),
                (
                    Cbor::Integer((-3).into()),
                    Cbor::Bytes(point.y().unwrap().to_vec()),
                ),
            ]);
            let mut cose_bytes = Vec::new();
            ciborium::ser::into_writer(&cose, &mut cose_bytes).unwrap();
            let mut ad = Sha256::digest(rp_id.as_bytes()).to_vec();
            ad.push(FLAG_UP | FLAG_AT);
            ad.extend_from_slice(&self.counter.to_be_bytes());
            ad.extend_from_slice(&[0u8; 16]);
            ad.extend_from_slice(&(self.cred_id.len() as u16).to_be_bytes());
            ad.extend_from_slice(&self.cred_id);
            ad.extend_from_slice(&cose_bytes);
            let att = Cbor::Map(vec![
                (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
                (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
                (Cbor::Text("authData".into()), Cbor::Bytes(ad)),
            ]);
            let mut att_bytes = Vec::new();
            ciborium::ser::into_writer(&att, &mut att_bytes).unwrap();
            CredentialResponse {
                id: B64.encode(&self.cred_id),
                raw_id: Some(B64.encode(&self.cred_id)),
                kind: Some("public-key".into()),
                response: AuthenticatorResponse {
                    client_data_json: B64.encode(Self::client_data(
                        "webauthn.create",
                        challenge,
                        origin,
                    )),
                    attestation_object: Some(B64.encode(att_bytes)),
                    authenticator_data: None,
                    signature: None,
                    user_handle: None,
                },
            }
        }

        pub fn assert(&mut self, options: &serde_json::Value, origin: &str) -> CredentialResponse {
            let challenge = options["challenge"].as_str().unwrap();
            let rp_id = options["rpId"].as_str().unwrap();
            self.counter += 1;
            let mut ad = Sha256::digest(rp_id.as_bytes()).to_vec();
            ad.push(FLAG_UP);
            ad.extend_from_slice(&self.counter.to_be_bytes());
            let cd = Self::client_data("webauthn.get", challenge, origin);
            let mut msg = ad.clone();
            msg.extend_from_slice(&Sha256::digest(&cd));
            let sig: Signature = self.key.sign(&msg);
            CredentialResponse {
                id: B64.encode(&self.cred_id),
                raw_id: Some(B64.encode(&self.cred_id)),
                kind: Some("public-key".into()),
                response: AuthenticatorResponse {
                    client_data_json: B64.encode(cd),
                    attestation_object: None,
                    authenticator_data: Some(B64.encode(ad)),
                    signature: Some(B64.encode(sig.to_der().as_bytes())),
                    user_handle: None,
                },
            }
        }
    }

    impl Default for SoftAuthenticator {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::soft::SoftAuthenticator;
    use super::*;

    fn rp() -> RelyingParty {
        RelyingParty {
            id: "horcrux.example".into(),
            origins: vec!["https://horcrux.example:8006".into()],
        }
    }

    #[test]
    fn register_then_authenticate() {
        let rp = rp();
        let mut a = SoftAuthenticator::new();
        let ch = new_challenge();
        let opts = creation_options(&rp, "uid", "alice", &ch, &[]);
        let resp = a.register(&opts, "https://horcrux.example:8006");
        let cred = verify_registration(&resp, &ch, &rp).unwrap();
        assert_eq!(cred.alg, COSE_ES256);

        let ch2 = new_challenge();
        let ropts = request_options(&rp, &ch2, std::slice::from_ref(&cred));
        let assertion = a.assert(&ropts, "https://horcrux.example:8006");
        let (idx, count) = verify_assertion(&assertion, &ch2, &rp, &[(cred.clone(), 0)]).unwrap();
        assert_eq!((idx, count), (0, 1));

        // replaying the same assertion (same counter) is refused
        assert!(verify_assertion(&assertion, &ch2, &rp, &[(cred.clone(), 1)]).is_err());
        // wrong challenge
        let other = new_challenge();
        let a2 = a.assert(&ropts, "https://horcrux.example:8006");
        assert!(verify_assertion(&a2, &other, &rp, &[(cred.clone(), 1)]).is_err());
    }

    #[test]
    fn rejects_phishing_origin_and_foreign_rp() {
        let rp = rp();
        let mut a = SoftAuthenticator::new();
        let ch = new_challenge();
        let opts = creation_options(&rp, "uid", "alice", &ch, &[]);
        let resp = a.register(&opts, "https://evil.example");
        assert!(verify_registration(&resp, &ch, &rp)
            .unwrap_err()
            .contains("origin"));

        let mut foreign = opts.clone();
        foreign["rp"]["id"] = json!("evil.example");
        let resp = a.register(&foreign, "https://horcrux.example:8006");
        assert!(verify_registration(&resp, &ch, &rp)
            .unwrap_err()
            .contains("RP ID"));
    }

    #[test]
    fn rejects_tampered_signature() {
        let rp = rp();
        let mut a = SoftAuthenticator::new();
        let ch = new_challenge();
        let cred = verify_registration(
            &a.register(&creation_options(&rp, "u", "a", &ch, &[]), &rp.origins[0]),
            &ch,
            &rp,
        )
        .unwrap();
        let ch2 = new_challenge();
        let mut assertion = a.assert(
            &request_options(&rp, &ch2, std::slice::from_ref(&cred)),
            &rp.origins[0],
        );
        // flip the counter byte in authenticatorData without re-signing
        let mut ad = b64(assertion.response.authenticator_data.as_ref().unwrap()).unwrap();
        ad[36] ^= 0x7f;
        assertion.response.authenticator_data = Some(B64.encode(ad));
        assert_eq!(
            verify_assertion(&assertion, &ch2, &rp, &[(cred, 0)]).unwrap_err(),
            "bad signature"
        );
    }

    #[test]
    fn rp_from_host_header() {
        let rp = RelyingParty::from_host("pve.lan:8006");
        assert_eq!(rp.id, "pve.lan");
        if std::env::var("HORCRUX_WEBAUTHN_ORIGIN").is_err() {
            assert_eq!(rp.origins, vec!["https://pve.lan:8006".to_string()]);
        }
        assert_eq!(RelyingParty::from_host("[fd00::1]:8006").id, "fd00::1");
    }
}
