"""OBS is the owner's personal app on HOGSMEADE only: it lives in the hogsmeade variant's local repo (package app-misc/hogsmeade-obs = the layer's set + the
machine config it needs) and nowhere public. Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus."""
import os
import re
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
LIVE = REPO / "build" / "live-iso"
HSM = LIVE / "project" / "variants" / "hogsmeade"
EB = HSM / "local-repo" / "app-misc" / "hogsmeade-obs" / "hogsmeade-obs-1.ebuild"


@unittest.skipUnless(EB.parent.parent.parent.parent.is_dir() or (REPO / "gentoo").is_dir(), "Horcrux repo tree only")
class HogsmeadeObs(unittest.TestCase):
    def setUp(self):
        if not EB.is_file():
            self.skipTest("Horcrux repo tree only")
        self.t = EB.read_text()

    def test_package_is_the_set_and_owns_the_config(self):
        for dep in ("media-video/obs-studio[nvenc,screencast,v4l]", "media-video/ffmpeg[opus,x264,nvenc]", "media-plugins/obs-pipewire-audio-capture", "media-video/v4l2loopback"):
            self.assertIn(dep, self.t)
        for f in ("package.accept_keywords", "package.use", "/etc/modprobe.d", "/etc/modules-load.d"):
            self.assertIn(f, self.t)
        self.assertIn("devices=1 video_nr=10", self.t)
        self.assertIn("exclusive_caps=1", self.t)
        self.assertIn("media-video/v4l2loopback dist-kernel", self.t)
        self.assertRegex(self.t, r"media-plugins/obs-pipewire-audio-capture ~amd64")

    def test_image_layer_installs_it_and_resolves_the_keywords(self):
        for d in ("Dockerfile", "Dockerfile.part2"):
            self.assertIn("app-misc/hogsmeade-obs", (HSM / d).read_text(), d)
            self.assertIn("local/app-misc/hogsmeade-obs/*.ebuild", (HSM / d).read_text(), d + ": its Manifest")
        kw = (HSM / "portage" / "package.accept_keywords" / "hogsmeade").read_text()
        self.assertIn("media-video/obs-studio ~amd64", kw)
        self.assertIn("media-plugins/obs-pipewire-audio-capture ~amd64", kw)
        use = (HSM / "portage" / "package.use" / "zzz-hogsmeade").read_text()
        self.assertIn("media-video/ffmpeg opus x264 nvenc", use)

    def test_not_in_any_other_machine_or_public_layer(self):
        pat = re.compile(r"obs-studio|hogsmeade-obs|v4l2loopback")
        roots = [REPO / "gentoo", LIVE / "common", LIVE / "project"]
        bad = []
        for r in roots:
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_obs.py":
                    continue
                if HSM in p.parents:
                    continue
                try:
                    if pat.search(p.read_text()):
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "OBS belongs to the HOGSMEADE layer only")


NC = HSM / "local-repo" / "app-misc" / "hogsmeade-nextcloud" / "hogsmeade-nextcloud-1.ebuild"


@unittest.skipUnless(NC.is_file(), "Horcrux repo tree only")
class HogsmeadeNextcloud(unittest.TestCase):
    """The Nextcloud desktop client is the owner's personal app: package-owned in the HOGSMEADE layer only, never in a public image or a lab test machine."""

    def test_package_owns_the_client_and_image_installs_it(self):
        t = NC.read_text()
        self.assertIn("net-misc/nextcloud-client[nautilus]", t)
        self.assertIn("package.use", t)
        for d in ("Dockerfile", "Dockerfile.part2"):
            txt = (HSM / d).read_text()
            self.assertIn("app-misc/hogsmeade-nextcloud", txt, d)
            self.assertIn("local/app-misc/hogsmeade-nextcloud/*.ebuild", txt, d + ": its Manifest")
        self.assertIn("net-misc/nextcloud-client nautilus", (HSM / "portage" / "package.use" / "zzz-hogsmeade").read_text())

    def test_not_leaked_into_public_or_test_machine_layers(self):
        # the package name exists nowhere outside the hogsmeade variant (and this test)
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_obs.py" or HSM in p.parents:
                    continue
                try:
                    if "hogsmeade-nextcloud" in p.read_text():
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "hogsmeade-nextcloud belongs to the HOGSMEADE layer only")
        # public images and lab test machines forbid the client outright
        forb = [l.strip() for l in (LIVE / "project" / "public-forbidden.txt").read_text().splitlines() if l.strip() and not l.startswith("#")]
        self.assertIn("net-misc/nextcloud-client", forb)
        deny = REPO / "gentoo" / "app-misc" / "horcrux-test-machine" / "files" / "personal-apps.denylist"
        if deny.is_file():
            self.assertIn("net-misc/nextcloud*", deny.read_text())
        # no active (non-comment) shared package list or public Dockerfile installs it
        shared = [LIVE / "project" / "packages.txt", LIVE / "project" / "packages-late.txt", LIVE / "common" / "packages.txt",
                  LIVE / "project" / "variants" / "gnome-light" / "packages.txt"]
        for f in shared:
            if f.is_file():
                self.assertNotRegex(f.read_text(), r"(?m)^[^#\n]*nextcloud", str(f))


APPS = HSM / "local-repo" / "app-misc" / "hogsmeade-apps" / "hogsmeade-apps-1.ebuild"
APPS_CFG = HSM / "local-repo" / "app-misc" / "hogsmeade-apps-config" / "hogsmeade-apps-config-1.ebuild"
STEAM_REPO = HSM / "local-repo" / "app-misc" / "hogsmeade-steam-repo" / "hogsmeade-steam-repo-1.ebuild"
STEAM = HSM / "local-repo" / "app-misc" / "hogsmeade-steam" / "hogsmeade-steam-1.ebuild"
OWNER_APPS = (
    "dev-util/claude-code", "app-editors/vscode", "dev-util/android-studio", "dev-java/openjdk-bin:25", "sys-apps/ripgrep", "dev-dotnet/dotnet-sdk-bin",
    "dev-util/github-cli", "dev-db/postgresql", "media-gfx/imagemagick", "app-misc/openrgb", "app-misc/solaar", "games-util/xpadneo", "sys-apps/bolt",
    "media-libs/libv4l[utils]", "net-print/hplip", "media-sound/easyeffects", "net-irc/halloy", "net-im/teams-for-linux", "net-im/signal-desktop-bin",
    "net-im/slack", "net-im/zoom", "net-im/proton-meet-bin", "mail-client/thunderbird-bin", "mail-client/proton-mail-bin", "games-emulation/dosbox-staging",
    "games-util/heroic-bin", "games-util/geforcenow-electron", "games-action/minecraft-launcher", "net-ftp/filezilla", "net-misc/freedownloadmanager",
    "net-fs/proton-drive-cli-bin", "media-video/makemkv", "media-gfx/loupe", "media-video/vlc", "net-analyzer/ookla-speedtest", "net-analyzer/wireshark",
    "net-analyzer/mtr", "net-analyzer/tcpdump", "net-dns/bind", "app-office/libreoffice", "app-crypt/yubioath-flutter-bin", "app-crypt/yubikey-manager",
    "sys-apps/pcsc-lite", "app-crypt/ccid", "sys-auth/proton-authenticator-bin", "app-admin/proton-pass-bin", "net-analyzer/caido-bin", "app-admin/awscli",
    "app-admin/aws-session-manager-plugin", "net-misc/s3cmd", "net-misc/onedrive", "www-client/google-chrome", "www-client/microsoft-edge", "net-vpn/proton-vpn-gtk-app",
    "app-misc/hogsmeade-steam",
)
NEVER = ("brave", "tailscale", "onlyoffice", "net-analyzer/zap", "etcher", "davinci")


@unittest.skipUnless(APPS.is_file(), "Horcrux repo tree only")
class HogsmeadeApps(unittest.TestCase):
    """The owner's Windows-inventory apps are ONE HOGSMEADE-only package set (app-misc/hogsmeade-apps) + Steam (app-misc/hogsmeade-steam[-repo])."""

    def test_set_lists_every_app_and_nothing_banned(self):
        t = APPS.read_text()
        for dep in OWNER_APPS:
            self.assertRegex(t, r"(?m)^\s*" + re.escape(dep) + r"\s*$", dep)
        self.assertIn("app-misc/hogsmeade-apps-config", t)
        live = "\n".join(l for l in (t + APPS_CFG.read_text() + STEAM.read_text() + STEAM_REPO.read_text()).lower().splitlines() if not l.lstrip().startswith("#"))
        for word in NEVER:
            self.assertNotIn(word, live, word)

    def test_config_package_owns_keywords_licences_and_use(self):
        t = APPS_CFG.read_text()
        for f in ("package.accept_keywords", "package.license", "package.use"):
            self.assertIn(f, t)
        for kw in ("net-im/slack ~amd64", "net-im/zoom ~amd64", "media-video/makemkv ~amd64", "dev-util/android-studio ~amd64", "app-office/libreoffice ~amd64"):
            self.assertIn(kw, t)
        for lic in ("net-im/slack *", "net-im/zoom *", "media-video/makemkv *", "www-client/google-chrome *", "www-client/microsoft-edge *", "dev-util/android-studio *"):
            self.assertIn(lic, t)
        self.assertIn("dev-util/claude-code cpu_flags_x86_avx cpu_flags_x86_avx2", t)

    def test_user_groups_and_pcscd_come_from_the_package(self):
        t = APPS.read_text()
        for g in ("pcap", "wireshark", "plugdev", "pcscd"):
            self.assertIn(g, t)
        self.assertIn("gpasswd -a", t)
        self.assertIn("rc-update add pcscd default", t)

    def test_steam_is_hogsmeade_only_with_32bit_deps(self):
        r = STEAM_REPO.read_text()
        self.assertIn("[steam-overlay]", r)
        self.assertIn("https://github.com/anyc/steam-overlay.git", r)
        self.assertIn("x11-drivers/nvidia-drivers abi_x86_32", r)
        self.assertIn("games-util/steam-launcher ~amd64", r)
        self.assertIn("ValveSteamLicense", r)
        self.assertIn("games-util/steam-launcher", STEAM.read_text())
        self.assertIn("app-misc/hogsmeade-steam-repo", STEAM.read_text())

    def test_not_leaked_and_public_forbidden(self):
        pat = re.compile(r"hogsmeade-apps|hogsmeade-steam|steam-overlay")
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_obs.py" or HSM in p.parents:
                    continue
                try:
                    if pat.search(p.read_text()):
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        # only the public-forbidden header comment may name the packages
        self.assertEqual([b for b in bad if not b.endswith("public-forbidden.txt")], [], "the owner's apps belong to the HOGSMEADE layer only")
        forb = {l.strip() for l in (LIVE / "project" / "public-forbidden.txt").read_text().splitlines() if l.strip() and not l.startswith("#")}
        for a in ("net-im/slack", "net-im/zoom", "net-im/signal-desktop-bin", "mail-client/thunderbird-bin", "mail-client/proton-mail-bin", "games-util/heroic-bin",
                  "games-action/minecraft-launcher", "media-video/makemkv", "www-client/google-chrome", "www-client/microsoft-edge", "games-util/steam-launcher"):
            self.assertIn(a, forb, a)
        # no active line of a shared or public package list installs any of them
        shared = [LIVE / "project" / "packages.txt", LIVE / "project" / "packages-late.txt", LIVE / "common" / "packages.txt",
                  LIVE / "project" / "variants" / "gnome-light" / "packages.txt"]
        for f in shared:
            if f.is_file():
                txt = f.read_text()
                for a in forb & set(OWNER_APPS) - {"games-util/steam-launcher"}:
                    self.assertNotRegex(txt, r"(?m)^[^#\n]*" + re.escape(a) + r"\b", f"{f}: {a}")


SP = HSM / "local-repo" / "app-misc" / "hogsmeade-speaker-tuning" / "hogsmeade-speaker-tuning-2.ebuild"


@unittest.skipUnless(SP.is_file(), "Horcrux repo tree only")
class HogsmeadeSpeakerTuning(unittest.TestCase):
    """The tuned-speaker PipeWire chain is for ONE laptop model (Alienware 18 Area-51 AA18250): package-owned in the HOGSMEADE layer only, DMI-gated."""

    def setUp(self):
        self.t = SP.read_text()

    def test_chain_is_highpass_compressor_brickwall_limiter(self):
        for s in ("bq_highpass", "http://lsp-plug.in/plugins/lv2/compressor_stereo", "http://lsp-plug.in/plugins/lv2/limiter_stereo",
                  "media-libs/lsp-plugins[lv2]", "media-video/pipewire[lv2", "Audio/Sink", "Speakers (tuned)", "HiFi__Speaker__sink"):
            self.assertIn(s, self.t)
        # limiter ceiling -1.5 dBFS (0.841 linear) with the gain-boost (output renormalisation) OFF and 2x+ oversampling: the offline measurement of
        # a -0.2 dBFS-mastered track (and the same track +6 dB) gave a TRUE peak of -1.4/-1.3 dBFS; the earlier -1.0 dBFS ceiling measured -0.9 dBFS true peak
        self.assertRegex(self.t, r'"th" = 0\.841\b')
        self.assertRegex(self.t, r'"ovs" = [2-9]\b')
        self.assertRegex(self.t, r'"boost" = 0\b')
        # total makeup at most +9 dB: compressor +6 (1.995) and limiter drive +3 (1.413)
        self.assertRegex(self.t, r'"mk" = 1\.995\b')
        self.assertRegex(self.t, r'"g_in" = 1\.413\b')
        # high-pass in the 120-150 Hz band
        freqs = {float(x) for x in re.findall(r'"Freq" = ([\d.]+)', self.t)}
        self.assertTrue(freqs and all(120 <= f <= 150 for f in freqs), freqs)
        # default only below headphones/USB: priority just above the speaker sink (712)
        self.assertRegex(self.t, r"priority\.session = 713\b")

    def test_default_output_policy_replaces_the_pin(self):
        """No hard pin; WirePlumber priority rules: tuned (713) default only while the speakers are the output; jack/USB/BT/explicit HDMI switch it; DGM20 output never."""
        import re as _re
        # nothing in the package sets a pinned default (only the elog that tells how to drop an old pin may mention the key)
        code = "\n".join(l for l in self.t.splitlines() if "elog" not in l and not l.lstrip("\t #").startswith("Default-output"))
        self.assertNotRegex(code, r"set-default|default\.configured\.audio\.sink\s*[=\"']|pw-metadata[^\n]*default\.configured[^\n]*\{")
        pol = self.t[self.t.index("51-hogsmeade-audio-policy.conf <<-"):]
        pol = pol[:pol.index("EOF2", 40)]
        rules = _re.findall(r'node\.name = "~([^"]+)"\s*\} \]\s*actions = \{ update-props = \{ priority\.session = (\d+)', pol)
        prio = {}
        for pat, n in rules:  # later rules win, like WirePlumber
            prio[pat] = int(n)
        spk, tuned = 712, 713
        self.assertEqual(int(_re.search(r"priority\.session = (\d+)", self.t).group(1)), tuned)
        self.assertGreater(prio["alsa_output.usb-*"], tuned)
        self.assertGreater(prio["bluez_output.*"], tuned)
        self.assertLess(prio["alsa_output.usb-Maono_DGM20_USB_Microphone_*"], spk)
        # the DGM20 demotion comes AFTER the generic USB rule (order matters), the headphone jack (UCM 1000) is untouched
        self.assertLess(pol.index("alsa_output.usb-*"), pol.index("alsa_output.usb-Maono_DGM20"))
        self.assertNotIn("Headphones", pol)
        # linked only on this model, like the chain
        self.assertIn("/etc/wireplumber/wireplumber.conf.d/51-hogsmeade-audio-policy.conf", self.t)
        self.assertNotIn("insinto /etc/wireplumber", self.t)
        self.assertIn('rm -f "$L" "$W"', self.t)

    def test_gated_on_this_model_only(self):
        self.assertIn('"Alienware"', self.t)
        self.assertIn('"Alienware 18 Area-51 AA18250"', self.t)
        self.assertIn("/sys/class/dmi/id", self.t)
        self.assertIn("rm -f", self.t)  # any other model: the link is removed
        # the chain is not installed into a directory PipeWire reads by default (that would load it on every machine)
        self.assertNotIn("insinto /usr/share/pipewire", self.t)
        self.assertNotIn("insinto /etc/pipewire", self.t)

    def test_image_layer_installs_and_enables_it(self):
        for d in ("Dockerfile", "Dockerfile.part2"):
            txt = (HSM / d).read_text()
            self.assertIn("app-misc/hogsmeade-speaker-tuning", txt, d)
            self.assertIn("local/app-misc/hogsmeade-speaker-tuning/*.ebuild", txt, d + ": its Manifest")
            self.assertIn("hogsmeade-speaker-tuning awccd", txt, d + ": OpenRC gate service")
        self.assertIn("media-video/pipewire lv2", (HSM / "portage" / "package.use" / "zzz-hogsmeade").read_text())

    def test_not_leaked_into_public_or_other_layers(self):
        pat = re.compile(r"hogsmeade-speaker-tuning|hogsmeade_speakers_tuned|50-hogsmeade-speakers|51-hogsmeade-audio-policy")
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == "test_hogsmeade_obs.py" or HSM in p.parents:
                    continue
                try:
                    if pat.search(p.read_text()):
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "the speaker tuning belongs to the HOGSMEADE layer only")


if __name__ == "__main__":
    unittest.main()
