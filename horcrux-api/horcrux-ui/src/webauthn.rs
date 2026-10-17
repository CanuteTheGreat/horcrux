//! WebAuthn browser ceremonies (security keys, passkeys). The server sends
//! options with base64url binaries; the browser API wants ArrayBuffers and
//! returns them, so a little JS converts both ways.

use crate::api::ApiError;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
function b64uToBuf(s) {
  s = s.replace(/-/g, '+').replace(/_/g, '/');
  while (s.length % 4) s += '=';
  const b = atob(s);
  const a = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) a[i] = b.charCodeAt(i);
  return a.buffer;
}
function bufToB64u(buf) {
  const a = new Uint8Array(buf);
  let s = '';
  for (let i = 0; i < a.length; i++) s += String.fromCharCode(a[i]);
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
export function hx_webauthn_available() {
  return !!(window.PublicKeyCredential && navigator.credentials);
}
export async function hx_webauthn_create(json) {
  const o = JSON.parse(json);
  o.challenge = b64uToBuf(o.challenge);
  o.user.id = b64uToBuf(o.user.id);
  (o.excludeCredentials || []).forEach(c => c.id = b64uToBuf(c.id));
  const c = await navigator.credentials.create({ publicKey: o });
  return JSON.stringify({
    id: c.id, rawId: bufToB64u(c.rawId), type: c.type,
    response: {
      clientDataJSON: bufToB64u(c.response.clientDataJSON),
      attestationObject: bufToB64u(c.response.attestationObject),
    },
  });
}
export async function hx_webauthn_get(json) {
  const o = JSON.parse(json);
  o.challenge = b64uToBuf(o.challenge);
  (o.allowCredentials || []).forEach(c => c.id = b64uToBuf(c.id));
  const c = await navigator.credentials.get({ publicKey: o });
  return JSON.stringify({
    id: c.id, rawId: bufToB64u(c.rawId), type: c.type,
    response: {
      clientDataJSON: bufToB64u(c.response.clientDataJSON),
      authenticatorData: bufToB64u(c.response.authenticatorData),
      signature: bufToB64u(c.response.signature),
      userHandle: c.response.userHandle ? bufToB64u(c.response.userHandle) : null,
    },
  });
}
"#)]
extern "C" {
    fn hx_webauthn_available() -> bool;
    #[wasm_bindgen(catch)]
    async fn hx_webauthn_create(options: String) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn hx_webauthn_get(options: String) -> Result<JsValue, JsValue>;
}

fn js_err(e: JsValue) -> ApiError {
    let msg = js_sys::Reflect::get(&e, &JsValue::from_str("message"))
        .ok()
        .and_then(|m| m.as_string())
        .or_else(|| e.as_string())
        .unwrap_or_else(|| "security key operation failed".into());
    ApiError {
        message: format!("Security key: {}", msg),
    }
}

/// Whether this browser supports WebAuthn (needs HTTPS or localhost).
pub fn available() -> bool {
    hx_webauthn_available()
}

/// `navigator.credentials.create` with server-provided options.
pub async fn create(options: &serde_json::Value) -> Result<serde_json::Value, ApiError> {
    let out = hx_webauthn_create(options.to_string())
        .await
        .map_err(js_err)?;
    serde_json::from_str(&out.as_string().unwrap_or_default()).map_err(|e| ApiError {
        message: e.to_string(),
    })
}

/// `navigator.credentials.get` with server-provided options.
pub async fn get(options: &serde_json::Value) -> Result<serde_json::Value, ApiError> {
    let out = hx_webauthn_get(options.to_string()).await.map_err(js_err)?;
    serde_json::from_str(&out.as_string().unwrap_or_default()).map_err(|e| ApiError {
        message: e.to_string(),
    })
}
