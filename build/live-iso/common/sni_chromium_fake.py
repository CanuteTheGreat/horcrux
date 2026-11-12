#!/usr/bin/env python3
"""Fake Chromium/Electron 43+ StatusNotifierItem.

Mimics the new Chromium SNI implementation: Properties.GetAll answers with only
the properties Chromium sets, and Properties.Get of any property it does not set
(IconPixmap, XAyatana*) fails with org.freedesktop.DBus.Error.Failed
"error occurred in Get" instead of returning an empty value.
Exit 0 if gnome-shell's StatusNotifierWatcher still lists the item after the
settle time; exit 1 if the extension dropped it.
Usage: fake_sni.py [settle_seconds]
"""
import os
import sys

import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

settle = float(sys.argv[1]) if len(sys.argv) > 1 else 12.0
EARLY = "--early" in sys.argv  # app was already running before the shell: never calls Register..., the extension must find it by bus scan
NAME = "org.freedesktop.StatusNotifierItem-%d-1" % os.getpid()
PATH = "/StatusNotifierItem"
SNI_XML = """<node><interface name="org.kde.StatusNotifierItem">
 <property name="Category" type="s" access="read"/>
 <property name="Id" type="s" access="read"/>
 <property name="Title" type="s" access="read"/>
 <property name="Status" type="s" access="read"/>
 <property name="WindowId" type="i" access="read"/>
 <property name="IconThemePath" type="s" access="read"/>
 <property name="Menu" type="o" access="read"/>
 <property name="ItemIsMenu" type="b" access="read"/>
 <property name="IconName" type="s" access="read"/>
 <property name="IconPixmap" type="a(iiay)" access="read"/>
 <property name="OverlayIconName" type="s" access="read"/>
 <property name="AttentionIconName" type="s" access="read"/>
 <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
 <property name="XAyatanaLabel" type="s" access="read"/>
 <property name="XAyatanaLabelGuide" type="s" access="read"/>
 <property name="XAyatanaOrderingIndex" type="u" access="read"/>
 <method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
 <signal name="NewIcon"/>
</interface></node>"""
PROPS_XML = """<node><interface name="org.freedesktop.DBus.Properties">
 <method name="Get"><arg type="s" direction="in"/><arg type="s" direction="in"/>
  <arg type="v" direction="out"/></method>
 <method name="GetAll"><arg type="s" direction="in"/>
  <arg type="a{sv}" direction="out"/></method>
</interface></node>"""

V = GLib.Variant
SET = {
    "Category": V("s", "ApplicationStatus"), "Id": V("s", "fake_status_icon_1"),
    "Title": V("s", ""), "Status": V("s", "Active"), "WindowId": V("i", 0),
    "IconThemePath": V("s", "/tmp"), "Menu": V("o", "/NO_DBUSMENU"),
    "ItemIsMenu": V("b", False), "IconName": V("s", "status_icon_0"),
    "OverlayIconName": V("s", ""), "AttentionIconName": V("s", ""),
    "ToolTip": V("(sa(iiay)ss)", ("", [], "No unread messages", "")),
}
loop = GLib.MainLoop()
result = {"ok": False}


def on_call(conn, sender, path, iface, method, params, inv):
    # Chromium/Electron 43+ only answers when the message is addressed to the
    # well-known name it registered; calls to its unique bus name (:1.x) fail.
    dest = inv.get_message().get_destination() or ""
    print("call %s.%s dest=%s sender=%s args=%s" % (iface, method, dest, sender, params.print_(False)), flush=True)
    if dest.startswith(":"):
        print("%s.%s to unique name %s -> ERROR" % (iface, method, dest), flush=True)
        inv.return_dbus_error("org.freedesktop.DBus.Error.Failed",
                              "error occurred in Get")
        return
    if iface == "org.freedesktop.DBus.Properties" and method == "GetAll":
        inv.return_value(V("(a{sv})", (SET,)))
    elif iface == "org.freedesktop.DBus.Properties" and method == "Get":
        prop = params.unpack()[1]
        print("Get", prop, "->", "value" if prop in SET else "ERROR", flush=True)
        if prop in SET:
            inv.return_value(V("(v)", (SET[prop],)))
        else:
            inv.return_dbus_error("org.freedesktop.DBus.Error.Failed",
                                  "error occurred in Get")
    else:
        inv.return_value(None)


def check():
    r = conn.call_sync("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher",
                       "org.freedesktop.DBus.Properties", "Get",
                       V("(ss)", ("org.kde.StatusNotifierWatcher",
                                  "RegisteredStatusNotifierItems")),
                       None, Gio.DBusCallFlags.NONE, 5000, None)
    items = [i for i in r.unpack()[0] if NAME in i]
    print("watcher items matching us:", items, flush=True)
    result["ok"] = bool(items)
    loop.quit()
    return False


conn = Gio.bus_get_sync(Gio.BusType.SESSION, None)
for xml in (SNI_XML, PROPS_XML):
    conn.register_object(PATH, Gio.DBusNodeInfo.new_for_xml(xml).interfaces[0],
                         on_call, None, None)
Gio.bus_own_name_on_connection(conn, NAME, Gio.BusNameOwnerFlags.NONE, None, None)
GLib.MainContext.default().iteration(False)
if EARLY:
    import time
    while not conn.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                             "NameHasOwner", V("(s)", ("org.kde.StatusNotifierWatcher",)), None,
                             Gio.DBusCallFlags.NONE, 5000, None).unpack()[0]:
        time.sleep(0.3)
else:
    conn.call_sync("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher",
                   "org.kde.StatusNotifierWatcher", "RegisterStatusNotifierItem",
                   V("(s)", (NAME,)), None, Gio.DBusCallFlags.NONE, 5000, None)
print("registered", NAME, flush=True)
GLib.timeout_add(int(settle * 1000), check)
loop.run()
sys.exit(0 if result["ok"] else 1)
