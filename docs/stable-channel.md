# Horcrux stable channel

Status: **not available yet.** Nothing is for sale, and the stable channel does not start before Horcrux 1.0 and a period of real use by people outside the project (see [ROADMAP.md](ROADMAP.md)).

## What it is

Horcrux is a Gentoo-based hypervisor for VMs and containers. Today everyone uses the **community channel**: the `main` branch rebuilt daily, free, with every feature. A **stable channel** is a second, slower stream of the same software for people who run Horcrux at work and want fewer surprises.

| | community channel | stable channel |
|---|---|---|
| What you get | the newest build, every day | a build that has already run for a while |
| Delay | none | at least 14 days behind community |
| Testing | automated tests on every build | the same tests, plus a two-week trial on test machines |
| Features | all | all, identical to community |
| Cost | free | paid yearly, once it exists |
| Support | public issue tracker | best-effort email support, see below |

The stable channel is not a different product and nothing is taken away from the community channel. It is the same code, picked at a known-good point in time.

## When it will exist

The stable channel starts only after all of these are true:

1. Horcrux 1.0 is released, signed and documented (the nine criteria in the [roadmap](ROADMAP.md)).
2. People other than the maintainers use Horcrux and have asked for a stable stream.
3. The terms of the offer and the support policy are published.
4. Upgrading from one stable release to the next, and rolling back, is tested automatically.

Until then the roadmap page is the place to watch; it lists each criterion with its current status.

## How a build becomes stable

1. Each week a recent community build is chosen as a candidate.
2. For at least 14 days it runs on test machines. It must pass the full automated matrix on every supported architecture: unattended and guided installs, upgrade from the previous stable release, rollback to it, reboots and power cuts, and security scans.
3. Only a candidate with no open serious bug is promoted. Promotion is signed.
4. A fix for a critical vulnerability may skip the 14-day wait but never the tests; the aim is to publish within 72 hours of a fix, best effort.
5. If a stable release turns out bad, the channel can be pointed back to the previous one in a single step.

Packages and package lists in the stable channel are signed. Details of the signing key and how it changes over time will be published with the first stable release.

## Support

Support for businesses is sold separately as yearly plans: best-effort email support during business hours, a first-reply target per plan, no guaranteed response or fix time, no 24/7 line. A plan buys support, not features. See the plans and what they include at [https://errant.solutions/support/horcrux/#plans](https://errant.solutions/support/horcrux/#plans).

Everyone, with or without a plan, can report bugs on the public issue tracker. Security problems go to the address on the [security page](https://horcrux.errant.solutions/security/).
