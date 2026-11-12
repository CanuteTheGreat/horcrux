# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (this one laptop model: Alienware 18 Area-51 AA18250; never in a public image, the Horcrux overlay or another machine).
# Windows loads Dell/Realtek speaker tuning (rt1320 "advanced mode" DSP firmware + Dolby) that Linux does not have, so at 0 dB everywhere the
# speakers are about half as loud. This installs the Linux equivalent as a PipeWire filter-chain: a virtual "Speakers (tuned)" sink in front of the
# internal speaker sink = high-pass (speaker protection) -> compressor (+6 dB makeup) -> lookahead brickwall limiter (+3 dB drive, -1.5 dBFS ceiling,
# 2x oversampled: measured true peak on a -0.2 dBFS-mastered track and on the same track +6 dB hotter stays at or below -1.3 dBFS), so the extra
# level never clips (replaces GNOME over-amplification, which clips).
# Default-output policy (no pinned default.configured.audio.sink; a WirePlumber rules drop-in instead): WirePlumber only considers sinks whose
# route is available, and picks the highest priority.session, so: tuned sink 713 (just above the raw speaker sink 712) is the default while the
# speakers are the output; the headphone jack (1000), any USB audio output and any Bluetooth sink (both forced to 1100) win when present and the
# default returns to the tuned speakers when they go away; HDMI/DP (~660) stays below and is used when picked in Settings (a configured default
# is honoured while its node exists). The Maono DGM20 microphone's own output sink is demoted to 50 so it never steals the default.
# The chain is only linked into PipeWire's config dir when DMI says this model (re-checked at every boot by the OpenRC service).
EAPI=8

DESCRIPTION="HOGSMEADE: speaker tuning (high-pass, compressor, -1 dBFS limiter) as a PipeWire filter-chain, gated on this laptop model"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

# the USE requirement mirrors the package.use file below (the image build resolves it from the variant's own portage/ files first)
RDEPEND="
	media-libs/lsp-plugins[lv2]
	media-video/pipewire[lv2,sound-server]
	media-video/wireplumber
"

src_install() {
	insinto /etc/portage/package.use
	newins - hogsmeade-speaker-tuning <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-speaker-tuning): PipeWire's filter-chain loads LV2 plugins only with lv2
		media-video/pipewire lv2
	EOF2

	insinto /usr/share/hogsmeade-speaker-tuning
	newins - 50-hogsmeade-speakers.conf <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-speaker-tuning): the "Speakers (tuned)" virtual sink in front of the internal speaker sink.
		# high-pass (speaker protection) -> compressor (+6 dB makeup) -> lookahead brickwall limiter (+3 dB drive, -1.5 dBFS ceiling so inter-sample true peaks stay under -1 dBFS).
		context.modules = [
		  { name = libpipewire-module-filter-chain
		    flags = [ nofail ]
		    args = {
		      node.description = "Speakers (tuned)"
		      media.name       = "Speakers (tuned)"
		      filter.graph = {
		        nodes = [
		          { type = builtin name = hpl1 label = bq_highpass control = { "Freq" = 130.0 "Q" = 0.707 "Gain" = 0.0 } }
		          { type = builtin name = hpl2 label = bq_highpass control = { "Freq" = 130.0 "Q" = 0.707 "Gain" = 0.0 } }
		          { type = builtin name = hpr1 label = bq_highpass control = { "Freq" = 130.0 "Q" = 0.707 "Gain" = 0.0 } }
		          { type = builtin name = hpr2 label = bq_highpass control = { "Freq" = 130.0 "Q" = 0.707 "Gain" = 0.0 } }
		          { type = lv2 plugin = "http://lsp-plug.in/plugins/lv2/compressor_stereo" name = comp
		            control = { "cm" = 0 "al" = 0.1 "cr" = 2.5 "kn" = 0.5 "at" = 15.0 "rt" = 200.0 "mk" = 1.995 "cdr" = 0.0 } }
		          { type = lv2 plugin = "http://lsp-plug.in/plugins/lv2/limiter_stereo" name = lim
		            control = { "g_in" = 1.413 "th" = 0.841 "lk" = 5.0 "at" = 5.0 "rt" = 5.0 "ovs" = 2 "boost" = 0 } }
		        ]
		        links = [
		          { output = "hpl1:Out" input = "hpl2:In" }
		          { output = "hpr1:Out" input = "hpr2:In" }
		          { output = "hpl2:Out" input = "comp:in_l" }
		          { output = "hpr2:Out" input = "comp:in_r" }
		          { output = "comp:out_l" input = "lim:in_l" }
		          { output = "comp:out_r" input = "lim:in_r" }
		        ]
		        inputs  = [ "hpl1:In" "hpr1:In" ]
		        outputs = [ "lim:out_l" "lim:out_r" ]
		      }
		      capture.props = {
		        node.name = "hogsmeade_speakers_tuned"
		        media.class = Audio/Sink
		        audio.channels = 2
		        audio.position = [ FL FR ]
		        priority.session = 713
		        priority.driver = 713
		      }
		      playback.props = {
		        node.name = "hogsmeade_speakers_tuned.output"
		        node.passive = true
		        audio.channels = 2
		        audio.position = [ FL FR ]
		        target.object = "alsa_output.pci-0000_80_1f.3-platform-sof_sdw.HiFi__Speaker__sink"
		      }
		    }
		  }
		]
	EOF2

	newins - 51-hogsmeade-audio-policy.conf <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-speaker-tuning): default-output policy as WirePlumber node-priority rules (highest priority.session among sinks
		# with an available route is the default; a sink picked in GNOME Settings is honoured while it exists). Rules run in order, later ones win.
		# Tuned speakers = 713 and the raw speaker sink = 712 (chain file); headphone jack = 1000, HDMI/DP ~ 650-670 (ACP/UCM values, untouched).
		monitor.alsa.rules = [
		  # any USB audio output (headset, DAC, dock) beats the speakers when connected
		  { matches = [ { node.name = "~alsa_output.usb-*" } ]
		    actions = { update-props = { priority.session = 1100  priority.driver = 1100 } } }
		  # ... except the Maono DGM20 microphone's own headphone-out sink, which must never steal the default
		  { matches = [ { node.name = "~alsa_output.usb-Maono_DGM20_USB_Microphone_*" } ]
		    actions = { update-props = { priority.session = 50  priority.driver = 50 } } }
		]
		monitor.bluez.rules = [
		  # a newly connected Bluetooth audio output beats the speakers
		  { matches = [ { node.name = "~bluez_output.*" } ]
		    actions = { update-props = { priority.session = 1100  priority.driver = 1100 } } }
		]
	EOF2

	exeinto /usr/libexec
	newexe - hogsmeade-speaker-tuning-gate <<-'EOF2'
		#!/bin/sh
		# Link the tuned-speaker chain into PipeWire's config dir on Alienware 18 Area-51 AA18250 only; remove the link anywhere else.
		D=/sys/class/dmi/id
		V=$(cat $D/sys_vendor 2>/dev/null); P=$(cat $D/product_name 2>/dev/null)
		L=/etc/pipewire/pipewire.conf.d/50-hogsmeade-speakers.conf
		W=/etc/wireplumber/wireplumber.conf.d/51-hogsmeade-audio-policy.conf
		if [ "$V" = "Alienware" ] && [ "$P" = "Alienware 18 Area-51 AA18250" ]; then
		  mkdir -p /etc/pipewire/pipewire.conf.d /etc/wireplumber/wireplumber.conf.d
		  ln -sfn /usr/share/hogsmeade-speaker-tuning/50-hogsmeade-speakers.conf "$L"
		  ln -sfn /usr/share/hogsmeade-speaker-tuning/51-hogsmeade-audio-policy.conf "$W"
		  echo "hogsmeade-speaker-tuning: enabled ($P)"
		else
		  rm -f "$L" "$W"
		  echo "hogsmeade-speaker-tuning: not this model ($V / $P), disabled"
		fi
	EOF2

	newinitd - hogsmeade-speaker-tuning <<-'EOF2'
		#!/sbin/openrc-run
		description="Enable the HOGSMEADE speaker-tuning PipeWire chain only on its own laptop model"
		depend() {
			need localmount
			before display-manager xdm gdm
		}
		start() {
			ebegin "Checking the machine model for the speaker-tuning chain"
			/usr/libexec/hogsmeade-speaker-tuning-gate
			eend 0
		}
	EOF2
}

pkg_postinst() {
	/usr/libexec/hogsmeade-speaker-tuning-gate
	if command -v rc-update >/dev/null 2>&1 && ! rc-update show default 2>/dev/null | grep -q hogsmeade-speaker-tuning; then
		rc-update add hogsmeade-speaker-tuning default
	fi
	elog "Restart the user's pipewire + wireplumber (or log in again) to load the 'Speakers (tuned)' sink and the default-output policy."
	elog "Drop an old hard pin with: pw-metadata -n default -d 0 default.configured.audio.sink (this package no longer pins the default)."
}

pkg_postrm() {
	rm -f /etc/pipewire/pipewire.conf.d/50-hogsmeade-speakers.conf /etc/wireplumber/wireplumber.conf.d/51-hogsmeade-audio-policy.conf
}
