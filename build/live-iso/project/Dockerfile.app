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
# Apple T2 Mac userland (Touch Bar daemon, fan daemon), pinned, portable CPU baseline. Own layer BEFORE `COPY . .`:
# it does not depend on the repository content. Only the binaries + their data/udev rules go into the image;
# the installer enables the services on a T2 Mac only (liveinst/t2.py).
RUN apt-get update && apt-get install -y --no-install-recommends libcairo2-dev libpango1.0-dev libgdk-pixbuf-2.0-dev libinput-dev libudev-dev \
        libgbm-dev libdrm-dev librsvg2-dev libglib2.0-dev git && rm -rf /var/lib/apt/lists/*
ARG TINY_DFR_REF=a79f2f43e6c6a9b855968e54eb243c20642ebc89
ARG T2FANRD_REF=48baf962697ec3d4d969c74cf601ee8e15b7aeaa
RUN --mount=type=cache,target=/cache/t2target --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git \
    set -e; mkdir -p /out/usr/bin /out/usr/share/tiny-dfr /out/usr/lib/udev/rules.d /w; \
    case "$(uname -m)" in x86_64) export RUSTFLAGS="-C target-cpu=x86-64-v2";; aarch64) export RUSTFLAGS="-C target-cpu=generic";; *) echo "no T2 userland on this arch"; exit 0;; esac; \
    export CARGO_TARGET_DIR=/cache/t2target; \
    git clone -q https://github.com/AsahiLinux/tiny-dfr /w/tiny-dfr && git -C /w/tiny-dfr checkout -q $TINY_DFR_REF; \
    git clone -q https://github.com/GnomedDev/T2FanRD /w/t2fanrd && git -C /w/t2fanrd checkout -q $T2FANRD_REF; \
    (cd /w/tiny-dfr && cargo build --release && cp /cache/t2target/release/tiny-dfr /out/usr/bin/ && cp share/tiny-dfr/* /out/usr/share/tiny-dfr/ && cp etc/udev/rules.d/*.rules /out/usr/lib/udev/rules.d/ && cp LICENSE* /out/usr/share/tiny-dfr/ 2>/dev/null || true); \
    (cd /w/t2fanrd && cargo build --release && cp /cache/t2target/release/t2fanrd /out/usr/bin/); \
    ls -l /out/usr/bin/tiny-dfr /out/usr/bin/t2fanrd
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
