# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

# Crates list generated with pycargoebuild

EAPI=8

CRATES="
	android_system_properties@0.1.5
	anyhow@1.0.102
	autocfg@1.5.0
	bitflags@2.11.0
	bumpalo@3.20.2
	bytemuck@1.25.0
	bytemuck_derive@1.10.2
	cairo-rs@0.22.0
	cairo-sys-rs@0.22.0
	cc@1.2.58
	cfg-expr@0.20.7
	cfg-if@1.0.4
	cfg_aliases@0.2.1
	chrono@0.4.44
	core-foundation-sys@0.8.7
	dirs-sys@0.4.1
	dirs@5.0.1
	drm-ffi@0.9.1
	drm-fourcc@2.2.0
	drm-sys@0.8.1
	drm@0.14.1
	equivalent@1.0.2
	errno@0.3.14
	find-msvc-tools@0.1.9
	freedesktop-icons@0.4.0
	freetype-rs@0.38.0
	freetype-sys@0.23.0
	futures-channel@0.3.32
	futures-core@0.3.32
	futures-executor@0.3.32
	futures-io@0.3.32
	futures-macro@0.3.32
	futures-task@0.3.32
	futures-util@0.3.32
	gdk-pixbuf-sys@0.22.0
	getrandom@0.2.17
	gio-sys@0.22.0
	gio@0.22.2
	glib-macros@0.22.2
	glib-sys@0.22.3
	glib@0.22.3
	gobject-sys@0.22.0
	hashbrown@0.16.1
	heck@0.5.0
	hermit-abi@0.3.9
	iana-time-zone-haiku@0.1.2
	iana-time-zone@0.1.65
	indexmap@2.13.0
	ini_core@0.2.0
	input-linux-sys@0.9.0
	input-linux@0.7.1
	input-sys@1.18.0
	input@0.9.1
	io-lifetimes@1.0.11
	js-sys@0.3.94
	libc@0.2.184
	libredox@0.1.15
	librsvg-rebind-sys@0.3.0
	librsvg-rebind@0.3.0
	libudev-sys@0.1.4
	libz-sys@1.1.25
	linux-raw-sys@0.12.1
	linux-raw-sys@0.4.15
	linux-raw-sys@0.9.4
	log@0.4.29
	memchr@2.8.0
	nix@0.29.0
	nix@0.30.1
	num-traits@0.2.19
	once_cell@1.21.4
	option-ext@0.2.0
	pin-project-lite@0.2.17
	pkg-config@0.3.32
	ppv-lite86@0.2.21
	privdrop@0.5.6
	proc-macro2@1.0.106
	pure-rust-locales@0.8.2
	quote@1.0.45
	rand@0.8.5
	rand_chacha@0.3.1
	rand_core@0.6.4
	redox_users@0.4.6
	rustix@0.38.44
	rustix@1.1.4
	rustversion@1.0.22
	serde@1.0.228
	serde_core@1.0.228
	serde_derive@1.0.228
	serde_spanned@0.6.9
	serde_spanned@1.1.1
	shlex@1.3.0
	slab@0.4.12
	smallvec@1.15.1
	syn@2.0.117
	system-deps@7.0.7
	target-lexicon@0.13.3
	thiserror-impl@1.0.69
	thiserror@1.0.69
	toml@0.8.23
	toml@0.9.12+spec-1.1.0
	toml_datetime@0.6.11
	toml_datetime@0.7.5+spec-1.1.0
	toml_edit@0.22.27
	toml_parser@1.1.2+spec-1.1.0
	toml_write@0.1.2
	toml_writer@1.1.1+spec-1.1.0
	tracing-attributes@0.1.31
	tracing-core@0.1.36
	tracing@0.1.44
	udev@0.9.3
	unicode-ident@1.0.24
	vcpkg@0.2.15
	version-compare@0.2.1
	wasi@0.11.1+wasi-snapshot-preview1
	wasm-bindgen-macro-support@0.2.117
	wasm-bindgen-macro@0.2.117
	wasm-bindgen-shared@0.2.117
	wasm-bindgen@0.2.117
	windows-core@0.62.2
	windows-implement@0.60.2
	windows-interface@0.59.3
	windows-link@0.2.1
	windows-result@0.4.1
	windows-strings@0.5.1
	windows-sys@0.48.0
	windows-sys@0.59.0
	windows-sys@0.61.2
	windows-targets@0.48.5
	windows-targets@0.52.6
	windows_aarch64_gnullvm@0.48.5
	windows_aarch64_gnullvm@0.52.6
	windows_aarch64_msvc@0.48.5
	windows_aarch64_msvc@0.52.6
	windows_i686_gnu@0.48.5
	windows_i686_gnu@0.52.6
	windows_i686_gnullvm@0.52.6
	windows_i686_msvc@0.48.5
	windows_i686_msvc@0.52.6
	windows_x86_64_gnu@0.48.5
	windows_x86_64_gnu@0.52.6
	windows_x86_64_gnullvm@0.48.5
	windows_x86_64_gnullvm@0.52.6
	windows_x86_64_msvc@0.48.5
	windows_x86_64_msvc@0.52.6
	winnow@0.7.15
	winnow@1.0.1
	xdg@2.5.2
	zerocopy-derive@0.8.48
	zerocopy@0.8.48
"

inherit cargo udev

DESCRIPTION="Dynamic function row (Touch Bar) daemon for Apple T2 / Apple Silicon Macs"
HOMEPAGE="https://github.com/AsahiLinux/tiny-dfr"
SRC_URI="
	https://github.com/AsahiLinux/tiny-dfr/archive/refs/tags/v${PV}.tar.gz -> ${P}.tar.gz
	${CARGO_CRATE_URIS}
"

LICENSE="Apache-2.0 MIT"
LICENSE+=" Apache-2.0-with-LLVM-exceptions ISC MIT MPL-2.0 Unicode-3.0"
SLOT="0"
KEYWORDS="~amd64"

DEPEND="
	x11-libs/cairo
	media-libs/freetype
	gnome-base/librsvg
	dev-libs/libinput
	virtual/libudev
	x11-libs/gdk-pixbuf
"
RDEPEND="${DEPEND}"
BDEPEND="virtual/pkgconfig"

QA_FLAGS_IGNORED="usr/bin/tiny-dfr"

src_install() {
	cargo_src_install
	insinto /usr/share/tiny-dfr
	doins -r share/tiny-dfr/.
	udev_dorules etc/udev/rules.d/99-touchbar-seat.rules
	newinitd "${FILESDIR}/tiny-dfr.initd" tiny-dfr
}
