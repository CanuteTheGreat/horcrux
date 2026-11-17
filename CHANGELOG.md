# Changelog

All notable changes to Horcrux. Versions follow [Semantic Versioning](https://semver.org/);
see [docs/UPDATE-POLICY.md](docs/UPDATE-POLICY.md) for what each kind of release means and how to update
or roll back. Generated from the commit history by `ci/changelog.py`; do not edit by hand.

## [0.1.0] - 2026-09-24

First tagged release.

### Features

- Enhance storage pool validation with comprehensive checks (6b02e845)
- Complete migration system with 100% real implementations (494ef678)

### Fixes

- Docker: add missing repo_name + generate ebuild Manifest for Gentoo build (e75f7c0d)
- Stop excluding gentoo/ from Docker build context (needed by Portage build) (2b417868)
- Auth: map login failures to AuthenticationFailed (401), not System (500) (c2ae15da)
- Match runtime base image glibc to builder (bookworm -> trixie) (d7029a8a)
- Login: wire login form to real auth API + JWT storage (b28b6feb)
- Add missing disks field to VmConfig test (8645dd92)
- Implement OIDC JWT signature verification (CRITICAL SECURITY FIX) (a1b5068b)
- Add missing disks field to VmConfig initialization (c9ffd967)

### Documentation

- Make Gentoo-first platform framing explicit for agents and humans (AGENTS.md, README, DEPLOYMENT, testing plan) (597b0bfd)
- Add comprehensive enhancements session summary (6f022b21)
- Add validation session summary (9a62129b)
- Add comprehensive test status report (37f83a1d)
- Add comprehensive OIDC security fix summary (9ae6b1cd)
- Update FINAL_STATUS.md - Platform now 5/5 stars, fully production ready (23cbc806)
- Update REMAINING_WORK.md to reflect OIDC security fix (c1a6b667)

### Other changes

- Fix library copying for horcrux binaries in ISO (73dcbc8a)
- Fix ISO boot with kernel modules and proper init system (b279b836)
- Add serial console support to ISO boot configuration (385e3e3b)
- Improve ISO build with initramfs library support and robustness (b5531304)
- Fix ISO build to include kernel and bootloader (c01076e4)
- Fix Docker build for updated project structure (8b67d536)
- Fix unused field warnings in scheduler UI (450fbd23)
- Add NAS features and standardize CLI output (eac7f004)
- Refactor Web UI into modular pages and add Gentoo build system (96b6470d)
- Added forgejo workflows. (0ff239a4)
- Storage, ha, and clustering improvements. (2a185b6d)
- Major platform enhancements and infrastructure cleanup (2d39b438)
- Add comprehensive noVNC testing suite and documentation (6c4b874c)
- Add comprehensive warning analysis and cleanup guide (24a46722)
- Update PROGRESS_SUMMARY with Docker API integration (f9ac9e01)
- Add complete Docker API integration with bollard (5b888434)
- Update roadmap with v0.1.1 completion status (89ddf22b)
- Add complete session documentation for testing and noVNC implementation (a35b763b)
- Complete production noVNC implementation with RFB protocol (3f76ac5a)
- Add comprehensive documentation for libvirt integration (4a3b05dd)
- Integrate libvirt with metrics collection system (8b9c01c0)
- Add libvirt integration for real VM metrics collection (dc9f81bd)
- Add noVNC WebSocket proxy for browser-based VM console access (d43feaf4)
- Add production-ready real-time metrics system (6e10cd16)
- Add complete installation and deployment system (9b86bac7)
- Add real-time monitoring and WebSocket features (616a72f1)
- Add comprehensive documentation for new features (5f34fbf5)
- Add comprehensive Web UI enhancements (37f0e88a)
- Add advanced SDN features: OVS and network templates (9381640f)
- Add comprehensive CLI enhancements (a84abc19)
- Add comprehensive OpenAPI 3.0 specification and Swagger UI (085608e5)
- Update README with current project statistics (d2fea775)
- Add comprehensive technical documentation (250fdca4)
- Add comprehensive API client examples and libraries (19468c3a)
- Add professional GitHub Pages website (26489beb)
- Add comprehensive Docker and container support (14c24728)
- Add comprehensive troubleshooting guide and configuration examples (905e80ec)
- Add comprehensive user documentation (af2f8c38)
- Add GitHub Actions CI/CD workflows (40de0677)
- Add comprehensive installation script (f9fa2759)
- Add comprehensive CONTRIBUTING.md guide (2901f39e)
- Fix doc comment syntax in storage backends (c54e1812)
- Suppress dead_code warnings for Phase 3 storage backends (9d772292)
- Suppress dead_code warnings for Phase 3 features (Batch 9) (158e3fa4)
- Add session summary for 2025-10-11 code quality improvements (943c8ca0)
- Fix Prometheus metric export formatting (5d8b05ff)
- Fix snapshot scheduler weekly frequency calculation (b8366466)
- Fix audit middleware event type determination logic (73922dab)
- Suppress dead_code warnings for Phase 3 enterprise features (673cde9a)
- Add #[allow(dead_code)] to validation module (1624321e)
- Fix field references after unused field rename (734d7e16)
- Fix unused field warnings across modules (aa2805f9)
- Remove unused imports across modules (6afaf9f1)
- Add comprehensive session completion summary (b09e51e7)
- Add comprehensive alert notifications configuration guide (438bc68e)
- Add comprehensive final validation report (5ca40c6e)
- Add comprehensive enhancement session summary (f2dac05a)
- Enhance alert notifications with native SMTP and HTTP (705e7d0f)
- Implement snapshot tree structure and S3 storage validation (35c2154e)
- Initial spaghetti. (69a92738)
- Fixed github urls; added .gitignore (3c5a0c60)
- First commit (da7859bf)
