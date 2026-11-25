#!/usr/bin/env python3
"""Capture one frame of the primary monitor through mutter's own ScreenCast API (org.gnome.Mutter.ScreenCast + PipeWire) into a
binary PPM: mutter-monitor-capture.py OUT.ppm

For VM tests whose virtual GPU is a GL one (virtio-vga-gl): QEMU's screendump/VNC cannot read a GL/blob scanout there, while the
compositor's own capture works on any renderer. Runs as the session user (needs DBUS_SESSION_BUS_ADDRESS and XDG_RUNTIME_DIR).
Prints "WxH" on success."""
import sys

import gi

gi.require_version("Gst", "1.0")
from gi.repository import Gio, GLib, Gst  # noqa: E402

BUS = "org.gnome.Mutter.ScreenCast"


def main(out):
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    sc = Gio.DBusProxy.new_sync(bus, 0, None, BUS, "/org/gnome/Mutter/ScreenCast", BUS, None)
    sp = sc.call_sync("CreateSession", GLib.Variant("(a{sv})", ({},)), 0, 10000).unpack()[0]
    sess = Gio.DBusProxy.new_sync(bus, 0, None, BUS, sp, BUS + ".Session", None)
    stp = sess.call_sync("RecordMonitor", GLib.Variant("(sa{sv})", ("", {"cursor-mode": GLib.Variant("u", 0)})), 0, 10000).unpack()[0]
    node = {}
    loop = GLib.MainLoop()

    def on_sig(_c, _s, _p, _i, sig, params):
        if sig == "PipeWireStreamAdded":
            node["id"] = params.unpack()[0]
            loop.quit()
    sub = bus.signal_subscribe(BUS, BUS + ".Stream", "PipeWireStreamAdded", stp, None, 0, on_sig)
    sess.call_sync("Start", None, 0, 10000)
    GLib.timeout_add_seconds(10, loop.quit)
    loop.run()
    bus.signal_unsubscribe(sub)
    try:
        if "id" not in node:
            raise SystemExit("no PipeWire stream from mutter")
        Gst.init(None)
        pipe = Gst.parse_launch(f"pipewiresrc path={node['id']} always-copy=true do-timestamp=true ! videoconvert ! "
                                "video/x-raw,format=RGB ! appsink name=s max-buffers=1 drop=false sync=false")
        sink = pipe.get_by_name("s")
        pipe.set_state(Gst.State.PLAYING)
        sample = None
        for _ in range(3):  # the first buffers can be empty while the stream negotiates
            sample = sink.emit("try-pull-sample", 10 * Gst.SECOND)
            if sample and sample.get_buffer().get_size():
                break
        pipe.set_state(Gst.State.NULL)
        if not sample:
            raise SystemExit("no frame from the PipeWire stream")
        st = sample.get_caps().get_structure(0)
        w, h = st.get_value("width"), st.get_value("height")
        buf = sample.get_buffer()
        ok, info = buf.map(Gst.MapFlags.READ)
        data = bytes(info.data)
        buf.unmap(info)
        stride = len(data) // h
        rows = b"".join(data[y * stride:y * stride + w * 3] for y in range(h))
        with open(out, "wb") as f:
            f.write(b"P6\n%d %d\n255\n" % (w, h) + rows)
        print(f"{w}x{h}")
    finally:
        try:
            sess.call_sync("Stop", None, 0, 5000)
        except GLib.Error:
            pass


if __name__ == "__main__":
    main(sys.argv[1])
