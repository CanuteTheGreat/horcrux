# Horcrux stable channel: design (not built)

Status: **design only**, written 2026-10-28. Nothing here is implemented, nothing is for sale, and none of it starts before Horcrux 1.0 and a period of real free use (see [ROADMAP.md](ROADMAP.md)). Prices and terms are proposals for the owner to decide.

Goal: a tested, delayed **stable** package stream for Horcrux (a Gentoo-based hypervisor (VMs and containers, web UI, clustering)), plus business-hours best-effort email support. The community channel stays complete and free.

## 1. Channels

| | community | stable |
|---|---|---|
| Source | `main`, rebuilt daily | a snapshot promoted from community |
| Delay | none | at least 14 days behind community |
| Testing | CI on every build, daily ISO tests | the candidate soak below, on top of CI |
| Access | anonymous, public | per-subscription credential |
| Features | all | all (identical) |
| Support | issue tracker | email, business hours, best effort |

Both channels ship the same code. Stable is a point-in-time selection of it, not a different product.

## 2. Promotion pipeline

1. **Candidate cut.** Weekly, a maintainer (or a scheduled job) picks a community build and tags `candidate-YYYY.WW`. The candidate is a complete, frozen set: `app-emulation/horcrux`, the pinned Gentoo dependency binpkgs it was built against, and the ISO.
2. **Soak (14 days minimum).** The candidate runs on the lab (Patronus lab VM on azkaban; the Horcrux laptops) with no P1 bug, and the full matrix passes on every supported architecture: unattended and guided installs, upgrade from the **previous stable** snapshot, rollback to it, reboot and power-cut survival, security scan (`cargo audit`, `cargo deny`, known-CVE check of the bundled Gentoo packages).
3. **Gate.** Promotion is a CI job that refuses to run unless: no open P1, all matrix runs green, soak elapsed, signed-off by a maintainer (a signed git tag `stable-<id>`).
4. **Publish.** The job copies the candidate into an immutable snapshot directory on the stable binhost, signs the package index, then atomically repoints `current`. Clients never see a half-published set.
5. **Security fast path.** A fix for a critical vulnerability may skip the 14-day soak but not the test matrix; target publication within 72 hours of a fixed build, best effort. The tag records that the soak was shortened.
6. **Rollback.** `current` can be repointed to the previous snapshot in one step. The last 6 snapshots are kept; clients can pin a snapshot id.

## 3. Binhost layout

Gentoo binary packages (gpkg format) served over HTTPS, one tree per architecture:

```
https://<stable-host>/horcrux/<arch>/            # arch = amd64 | arm64
  current            -> snapshots/2027.03   # moved atomically by the gate job
  snapshots/
    2027.03/
      Packages       # index, covers this snapshot only
      Packages.sig   # detached signature of the index
      app-emulation/ ... gpkg files (hash-addressed duplicates hardlinked)
      MANIFEST.json  # candidate id, git commit, test evidence URL, soak dates
    2027.02/ ...
  keys/
    horcrux-stable.gpg   # current public signing key + previous during overlap
```

Client configuration (written by an `activate` helper, `0600`):

```
# /etc/portage/binrepos.conf/horcrux-stable.conf
[horcrux-stable]
priority = 10
sync-uri = https://<stable-host>/horcrux/amd64/current
verify-signature = true
```

with `FEATURES="binpkg-signing"` verification against the key in the keyring. Portage's handling of credentials in `sync-uri` and of the `current` indirection must be confirmed in a prototype before this layout is final; it is the main unverified assumption in this document.

Open question, decide before building: where the files live. Two candidates, both free to try and neither bought now: object storage the owner already uses (Backblaze B2) or Cloudflare R2, fronted by a small Cloudflare Worker that checks credentials. Cloudflare's terms limit serving large non-HTML files through the plain CDN unless it is via its storage products, so verify that against current terms before choosing a design that proxies bulk binaries.

## 4. Authentication

- One **subscription key** per Stripe subscription (32 random bytes, shown once; only a hash is stored).
- Sent as HTTP basic auth (`user = subscription id`, `password = key`). The edge (Worker) looks the hash up in an entitlement store and returns 401/403 otherwise. Public paths: `keys/` only.
- The key licenses **host (node)** count as stated in the order. Enforcement is soft: the Worker counts distinct client IPs per key per 30 days and flags (does not block) large overages for a human email. No telemetry beyond server access logs.
- Rate limits per key. Logs hold the subscription id, path and status; retention 30 days.
- Failed payment: Stripe retries; access stays for a 14-day grace, then suspends. Cancellation: access to the end of the paid period. Suspended keys fall back to the free community channel, which still works.

## 5. Stripe wiring (test mode until phase 3)

- Product + recurring Price per tier; checkout through Stripe Checkout, self-service changes through the Stripe Customer Portal. Use the existing Errant Solutions account in **test mode only** until the owner says otherwise.
- Webhook endpoint (signature verified, idempotent on event id): `checkout.session.completed` creates the subscription record and key, then emails the activation instructions; `invoice.paid` extends the entitlement; `invoice.payment_failed` starts the grace; `customer.subscription.deleted` revokes at period end.
- Keys are emailed through the Errant mail flow (Resend, sending from an `errant.solutions` address). Nothing is sent from this repo.
- Tax and invoicing (EU VAT, US sales tax) need a decision; Stripe Tax is one option and has its own fees. Flag for the accountant.
- Where the issuer runs is an owner decision; a Worker plus a small key/value store avoids a new server.

## 6. Key rotation

| Secret | Rotation | Procedure |
|---|---|---|
| Package/index signing key | Subkey every 12 months, primary offline | Create new subkey, publish both public keys in `keys/` and in the overlay's keyring package for 90 days, sign with the new one, retire the old one. Keep a pre-made revocation certificate offline. A compromise means revoke, re-sign the current snapshot, ship an emergency keyring update through the **community** channel too. |
| Subscriber key | On request, on leak, and optionally yearly | Customer Portal link or support request issues a new key; the old key works 24 hours more, then is deleted. |
| Stripe webhook secret and API key | Every 12 months and on staff change | Roll in the Stripe dashboard, update the issuer's secret store, confirm a test event, then revoke the old one. |
| CI deploy/signing keys | Every 12 months | Same pattern the Errant sites already use (ed25519 signing key in a repo secret, public half allow-listed on the host). |

## 7. Support

- Inbox: a product support address on `errant.solutions`, delivered through the existing Errant mail flow. Subscribers are identified by subscription id in the subject or by the sending address on the order.
- Policy to publish before the first sale: business hours (stated with a time zone), best-effort first reply, no guaranteed response time, no 24/7, scope = installation, upgrades and bugs in the stable channel on supported versions and hardware; out of scope = custom development, third-party software, unsupported versions, consulting.
- Security issues: a separate, public disclosure address and policy (part of criterion 9).

## 8. Terms (needs a lawyer)

A short Terms of Service is required before the first sale, covering at least: what is sold (access to a package stream and best-effort support, not a warranty), disclaimer of warranties, **limitation of liability**, refunds and cancellation, acceptable use and key sharing, privacy (what logs are kept), governing law, and tax. **This must be reviewed by a lawyer; this document is not legal advice and proposes no final wording.** Also for the lawyer: the software is GPL-3.0, so a subscription must not restrict the rights the licence gives to recipients of the binaries; the model sells the tested stream, the convenience and the support, and the source stays public.

## 9. Pricing benchmarks (checked 2026-10-28)

| Vendor / plan | Price | What it includes | Source |
|---|---|---|---|
| Proxmox VE Community | EUR 120 / CPU socket / year | Enterprise (stable) repository, no ticket support | [proxmox.com pricing](https://www.proxmox.com/en/products/proxmox-virtual-environment/pricing) |
| Proxmox VE Basic | EUR 370 / socket / year | 3 tickets per year, 1 business day response | same |
| Proxmox VE Standard | EUR 550 / socket / year | 10 tickets, 4 hour response, 24/7 (phased in by end of 2026) | same |
| Proxmox VE Premium | EUR 1,100 / socket / year | unlimited tickets, 2 hour response, 24/7 | same |
| Netgate TAC Lite | US$129 / year | pfSense+ updates, portal; no 24/7 | [netgate.com/support](https://www.netgate.com/support) |
| Netgate TAC Pro | US$399 / year | 24/7, 24 hour target initial response | same |
| Netgate TAC Enterprise | US$799 / year | 24/7, 4 hour target response | same |
| OPNsense Business Edition | EUR 149 / year (3 years: EUR 399 on sale) | commercial firmware repository with a more selective upgrade path; one installation per license | [shop.opnsense.com](https://shop.opnsense.com/product/opnsense-business-edition/) |
| OPNsense Business Support | EUR 329 / year | email and phone, 5x8 CET business hours, 2 hours of implementation help included, 20% off with a Business Edition license | [shop.opnsense.com](https://shop.opnsense.com/product/opnsense-business-support-subscription/) |

Read-across: the market sells a stable repo alone for roughly EUR/US$ 120 to 150 per unit per year, and a repo with real support at EUR 330 to 400 or more. Every vendor above is established with a team and, at the upper tiers, a guaranteed response. Horcrux would offer best effort from a pre-1.0-then-young project, so it should price below the supported tiers.

**Suggested starting range: US$149 to US$249 per host per year; start at US$199 with a US$149 first-year launch price for the first 50 hosts.** Proxmox VE sells a repo-only Community subscription at EUR 120 per socket per year and Basic (1 business day response, 3 tickets) at EUR 370. Horcrux's offer sits between them: repo plus best-effort email, no response guarantee, from a younger project. Pricing per host rather than per socket keeps it simple to count and to bill. Mixed-architecture clusters mean per-socket pricing would be awkward (ARM boards have one socket and cost little), another reason to price per host.

These numbers are a hypothesis. Validate by asking the first real free users what they would pay, and re-check the vendors' pages before publishing any price. Prices change (Proxmox's page already announces changes effective 2026-10-19 and end of 2026).

## 10. What is needed before building any of this

1. Horcrux 1.0 released and signed (criteria 1 to 9 in [ROADMAP.md](ROADMAP.md)).
2. Real free users who have asked for it.
3. Lawyer-reviewed terms; accountant decision on tax.
4. A prototype proving the Portage assumptions in section 3 (credentials in `sync-uri`, `current` indirection, signature verification) against a throwaway binhost.
