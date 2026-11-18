"""horcrux-test-machine: wired default-route repair in the netwatch script (fake nmcli/ip, no root) and the line-buffered klog service."""
import os, pathlib, re, subprocess, tempfile, unittest

FILES = pathlib.Path(__file__).resolve().parent.parent / "files"


class WiredRoute(unittest.TestCase):
    def run_check(self, route_out, never_default="no"):
        src = (FILES / "horcrux-test-netwatch.sh").read_text()
        fn = re.search(r"^WIRED_FIX=.*?^}\n", src, re.S | re.M).group(0)
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            (d / "nmcli").write_text(
                '#!/bin/bash\necho "nmcli $*" >> "$NWLOG"\ncase "$*" in\n'
                ' *DEVICE,TYPE,STATE*) echo eno1:ethernet:connected;;\n *GENERAL.CONNECTION*) echo "Wired 1";;\n'
                ' *never-default*) echo "$NEVER";;\nesac\n')
            (d / "ip").write_text(
                '#!/bin/bash\ncase "$*" in *"addr show"*) echo "2: eno1 inet 10.0.0.5/24 scope global eno1";;\n'
                ' *"route show default"*) echo "$ROUTE";; esac\n')
            for n in ("nmcli", "ip"):
                (d / n).chmod(0o755)
            (d / "t.sh").write_text('say(){ :; }\nsleep(){ :; }\n' + fn + "wired_check\nwired_check\n")
            env = dict(os.environ, PATH=f"{d}:{os.environ['PATH']}", NWLOG=str(d / "calls"), ROUTE=route_out, NEVER=never_default)
            subprocess.run(["bash", str(d / "t.sh")], env=env, check=True)
            calls = (d / "calls").read_text() if (d / "calls").exists() else ""
        return calls.count("connection up")

    def test_routeless_wired_is_reactivated_on_second_sighting(self):
        self.assertEqual(self.run_check(""), 1)

    def test_wired_with_default_route_is_left_alone(self):
        self.assertEqual(self.run_check("default via 10.0.0.1 dev eno1"), 0)

    def test_never_default_connection_is_left_alone(self):
        self.assertEqual(self.run_check("", never_default="yes"), 0)


class KlogLineBuffered(unittest.TestCase):
    def test_both_inits_use_stdbuf(self):
        self.assertIn("stdbuf -oL", (FILES / "horcrux-test-klog.service").read_text())
        rc = (FILES / "horcrux-test-klog.initd").read_text()
        self.assertIn("command=/usr/bin/stdbuf", rc)
        self.assertIn("-oL /bin/dmesg", rc)


if __name__ == "__main__":
    unittest.main()
