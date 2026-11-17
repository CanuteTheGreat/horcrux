# Security policy

## Reporting a vulnerability

Please report security problems in Horcrux privately by e-mail to **security@errant.solutions**. Do not open a public issue for a
vulnerability. If you want to encrypt your report, use the OpenPGP key published at
https://horcrux.errant.solutions/signing-key.asc (fingerprint below).

Include what you found, the version or daily build (`/etc/os-release` `BUILD_ID`, or the ISO file name and date), and the
steps to reproduce it. You get an answer within 3 working days. We agree a disclosure date with you once a fix exists, and we
credit you in the release notes unless you prefer not to be named.

The contact is also in https://horcrux.errant.solutions/.well-known/security.txt (RFC 9116).

## Supported versions

Horcrux is pre-1.0. Only the newest build is supported: the rolling `daily` build from `main`, and, once tagged releases exist,
the newest release. Fixes land on `main` and ship in the next daily build. Older daily builds get no fixes: update or reinstall.

| Version | Supported |
| --- | --- |
| newest daily build (`main`) | yes |
| older daily builds | no |

## Verifying downloads

Every published file (ISO images (server and desktop editions, OpenRC and systemd), the manager tarball, the `.sha256` checksum files, the CycloneDX SBOM `.cdx.json` and the test evidence) has a
detached OpenPGP signature (`.asc`) made with the Errant Solutions release key:

    Errant Solutions Release Signing <security@errant.solutions>
    ed25519, fingerprint 5439 3549 39B9 C055 2A68  E9BB E323 A6A3 AC0A 6506

The same public key is in this repository (`keys/release-signing.asc`), on https://horcrux.errant.solutions/signing-key.asc,
and installed on Horcrux systems by the package `sec-keys/openpgp-keys-errantsolutions` (`/usr/share/openpgp-keys/errantsolutions.asc`).

    gpg --import signing-key.asc
    gpg --verify FILE.asc FILE

A file whose signature does not verify must not be used. The CI publish step signs every file and refuses to publish anything it
could not sign and verify.
