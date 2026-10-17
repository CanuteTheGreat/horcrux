//! HTTPS certificate: what's live, regenerate self-signed, upload, ACME.

use crate::api::{self, AcmeConfig, TlsStatus};
use leptos::*;

#[component]
pub fn CertificatesPage() -> impl IntoView {
    let (status, set_status) = create_signal(None::<TlsStatus>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (cert_pem, set_cert_pem) = create_signal(String::new());
    let (key_pem, set_key_pem) = create_signal(String::new());
    let (san, set_san) = create_signal(String::new());
    let (acme_dir, set_acme_dir) =
        create_signal("https://acme-v02.api.letsencrypt.org/directory".to_string());
    let (acme_email, set_acme_email) = create_signal(String::new());
    let (acme_domains, set_acme_domains) = create_signal(String::new());
    let (acme_ca, set_acme_ca) = create_signal(String::new());
    let (acme_tos, set_acme_tos) = create_signal(false);
    let (busy, set_busy) = create_signal(false);

    let reload = move || {
        spawn_local(async move {
            match api::get_tls_status().await {
                Ok(s) => {
                    if let Some(a) = &s.acme {
                        set_acme_dir.set(a.directory_url.clone());
                        set_acme_email.set(a.contact_email.clone().unwrap_or_default());
                        set_acme_domains.set(a.domains.join(", "));
                        set_acme_ca.set(a.ca_cert.clone().unwrap_or_default());
                        set_acme_tos.set(a.accept_tos);
                    }
                    set_status.set(Some(s));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    reload();

    let done = move |r: Result<api::CertificateInfo, api::ApiError>, what: &'static str| {
        set_busy.set(false);
        match r {
            Ok(c) => {
                set_notice.set(Some(format!(
                    "{}: now serving {} (valid until {})",
                    what, c.subject, c.valid_until
                )));
                reload();
            }
            Err(e) => set_error.set(Some(e.message)),
        }
    };

    let regenerate = move |_| {
        set_error.set(None);
        set_busy.set(true);
        let names: Vec<String> = san
            .get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        spawn_local(async move {
            done(
                api::regenerate_self_signed(names).await,
                "Local CA certificate",
            )
        });
    };
    let upload = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        set_busy.set(true);
        let (c, k) = (cert_pem.get(), key_pem.get());
        spawn_local(
            async move { done(api::upload_certificate(c, k).await, "Uploaded certificate") },
        );
    };
    let save_acme = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        let cfg = AcmeConfig {
            directory_url: acme_dir.get(),
            contact_email: Some(acme_email.get()).filter(|e| !e.is_empty()),
            domains: acme_domains
                .get()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            ca_cert: Some(acme_ca.get()).filter(|e| !e.is_empty()),
            auto_renew: true,
            renew_days_before: 30,
            accept_tos: acme_tos.get(),
        };
        spawn_local(async move {
            match api::save_acme_config(&cfg).await {
                Ok(()) => {
                    set_notice.set(Some("ACME settings saved".into()));
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    let order = move |_| {
        set_error.set(None);
        set_busy.set(true);
        spawn_local(async move { done(api::order_acme_certificate().await, "ACME certificate") });
    };

    view! {
        <div class="page certificates-page">
            <h1>"Certificates"</h1>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="cert-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="cert-notice">{n}</div> })}
            {move || status.get().map(|s| {
                let c = s.certificate.clone();
                view! {
                    <section class="card" id="cert-current">
                        <h2>"Serving now"</h2>
                        <table class="table">
                            <tr><th>"Subject"</th><td id="cert-subject">{c.subject.clone()}</td></tr>
                            <tr><th>"Issuer"</th><td>{c.issuer.clone()}</td></tr>
                            <tr><th>"Names"</th><td id="cert-san">{c.san.join(", ")}</td></tr>
                            <tr><th>"Valid"</th><td>{format!("{} - {}", c.valid_from, c.valid_until)}</td></tr>
                            <tr><th>"SHA-256"</th><td><code id="cert-fingerprint">{c.fingerprint_sha256.clone()}</code></td></tr>
                            <tr><th>"Source"</th><td id="cert-source">{c.source.clone()}</td></tr>
                        </table>
                        {s.acme_last_result.clone().map(|r| view! {
                            <p id="acme-last">{format!("Last ACME run: {}", r)}</p>
                        })}
                    </section>
                }
            })}

            <section class="card">
                <h2>"Local CA"</h2>
                <p>
                    "Out of the box this node issues its own certificate from its local CA. "
                    "Import the CA once and browsers trust this node: "
                    <a href="/api/tls/ca.pem" id="download-ca" download="horcrux-local-ca.pem">"download horcrux-local-ca.pem"</a>
                    ". Re-issue below to add names (e.g. the address you browse to)."
                </p>
                <input type="text" id="cert-extra-san" placeholder="extra names: pve.lan, 10.0.0.5"
                    on:input=move |ev| set_san.set(event_target_value(&ev)) prop:value=san/>
                <button class="btn" id="cert-regenerate" on:click=regenerate disabled=busy>"Issue new certificate"</button>
            </section>

            <section class="card">
                <h2>"Upload certificate"</h2>
                <form id="cert-upload" on:submit=upload>
                    <label>"Certificate chain (PEM, leaf first)"</label>
                    <textarea id="cert-pem" rows="6" on:input=move |ev| set_cert_pem.set(event_target_value(&ev)) prop:value=cert_pem></textarea>
                    <label>"Private key (PEM)"</label>
                    <textarea id="key-pem" rows="6" on:input=move |ev| set_key_pem.set(event_target_value(&ev)) prop:value=key_pem></textarea>
                    <button type="submit" class="btn btn-primary" id="cert-upload-submit" disabled=busy>"Install"</button>
                </form>
            </section>

            <section class="card">
                <h2>"ACME (Let's Encrypt)"</h2>
                <p>
                    "The CA checks " <code>"http://<domain>/.well-known/acme-challenge/"</code>
                    " on port 80: start Horcrux with HORCRUX_HTTP_REDIRECT_PORT=80 (or forward port 80 to that port). "
                    {move || status.get().map(|s| match s.config["http_redirect_port"].as_u64() {
                        Some(p) => format!("Plain-HTTP port: {}.", p),
                        None => "No plain-HTTP port is configured right now.".to_string(),
                    })}
                </p>
                <form id="acme-form" on:submit=save_acme>
                    <label>"Directory URL"</label>
                    <input type="text" id="acme-directory" on:input=move |ev| set_acme_dir.set(event_target_value(&ev)) prop:value=acme_dir/>
                    <label>"Contact email"</label>
                    <input type="email" id="acme-email" on:input=move |ev| set_acme_email.set(event_target_value(&ev)) prop:value=acme_email/>
                    <label>"Domains (comma separated)"</label>
                    <input type="text" id="acme-domains" on:input=move |ev| set_acme_domains.set(event_target_value(&ev)) prop:value=acme_domains/>
                    <label>"Extra CA for the ACME server (PEM path, private CAs only)"</label>
                    <input type="text" id="acme-ca" on:input=move |ev| set_acme_ca.set(event_target_value(&ev)) prop:value=acme_ca/>
                    <label class="checkbox">
                        <input type="checkbox" id="acme-tos" prop:checked=acme_tos on:change=move |_| set_acme_tos.update(|v| *v = !*v)/>
                        " I accept the CA's terms of service"
                    </label>
                    <button type="submit" class="btn" id="acme-save">"Save"</button>
                    <button type="button" class="btn btn-primary" id="acme-order" on:click=order disabled=busy>"Order certificate now"</button>
                </form>
            </section>
        </div>
    }
}
