# syntax=docker/dockerfile:1.4
# Builds the Horcrux binaries + web UI on Debian bookworm (glibc 2.36 is
# older than Gentoo's, so the binaries run on the Gentoo live system) and
# stages them under /out. Portable CPU baseline (x86-64-v2 / generic arm64 / rv64gc), never target-cpu=native:
# a native build on the CI host crashed with SIGILL on other CPUs; check-cpu-baseline.py fails the build if it regresses. Context = repository root.
FROM rust:1-bookworm AS app
RUN apt-get update && apt-get install -y --no-install-recommends clang libvirt-dev libpam0g-dev pkg-config curl ca-certificates && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
# static (musl) build: the glibc one needs a newer libc than bookworm
RUN curl -fsSL https://github.com/trunk-rs/trunk/releases/latest/download/trunk-$(uname -m)-unknown-linux-musl.tar.gz | tar xz -C /usr/local/bin
WORKDIR /src
COPY . .
ENV CARGO_TARGET_DIR=/cache/target CARGO_PROFILE_RELEASE_DEBUG=0
ARG JOBS=4
RUN --mount=type=cache,target=/cache/target --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    set -e; export CARGO_BUILD_JOBS=${JOBS}; \
    case "$(uname -m)" in x86_64) cpu=x86-64-v2; trip=X86_64_UNKNOWN_LINUX_GNU;; aarch64) cpu=generic; trip=AARCH64_UNKNOWN_LINUX_GNU;; riscv64) cpu=generic-rv64; trip=RISCV64GC_UNKNOWN_LINUX_GNU;; *) echo "unsupported arch"; exit 1;; esac; \
    export CARGO_TARGET_${trip}_RUSTFLAGS="-C target-cpu=$cpu"; \
    cargo build --release -p horcrux-api -p horcrux-cli; \
    (cd horcrux-api/horcrux-ui && CARGO_TARGET_DIR=/cache/target-wasm trunk build --release); \
    mkdir -p /out/usr/bin /out/usr/share/horcrux/ui; \
    cp /cache/target/release/horcrux /cache/target/release/horcrux-api /out/usr/bin/; \
    python3 build/live-iso/common/check-cpu-baseline.py /out/usr/bin/horcrux /out/usr/bin/horcrux-api; \
    cp -r horcrux-api/horcrux-ui/dist/. /out/usr/share/horcrux/ui/; \
    mkdir -p /out/etc/init.d /out/etc/conf.d
COPY gentoo/app-emulation/horcrux/files/horcrux-guests.initd /out/etc/init.d/horcrux-guests
FROM scratch
COPY --from=app /out/ /out/
