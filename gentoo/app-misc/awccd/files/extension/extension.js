// GNOME Shell extension: panel menu for awccd (org.horcrux.Awcc1). No code from Dell; clean-room.
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const IFACE = `<node><interface name="org.horcrux.Awcc1">
  <method name="GetStatus"><arg type="s" direction="out"/></method>
  <method name="SetProfile"><arg type="s" direction="in"/><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
  <method name="LightingStatic"><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
  <method name="LightingRestore"><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
  <signal name="Changed"><arg type="s"/></signal>
</interface></node>`;
const Proxy = Gio.DBusProxy.makeProxyWrapper(IFACE);
const COLORS = {Blue: '0000FF', Red: 'FF0000', Green: '00FF00', White: 'FFFFFF', Purple: '8000FF'};

export default class AwccExtension extends Extension {
    enable() {
        this._btn = new PanelMenu.Button(0.0, 'Alienware', false);
        this._label = new St.Label({text: 'AW', y_align: 2});
        this._btn.add_child(this._label);
        this._proxy = new Proxy(Gio.DBus.system, 'org.horcrux.Awcc1', '/org/horcrux/Awcc1', (p, err) => {
            if (err) { this._label.text = 'AW?'; return; }
            this._sig = p.connectSignal('Changed', () => this._refresh());
            this._refresh();
        });
        this._profiles = new PopupMenu.PopupSubMenuMenuItem('Thermal profile');
        this._fans = new PopupMenu.PopupMenuItem('fans', {reactive: false});
        this._btn.menu.addMenuItem(this._profiles);
        this._btn.menu.addMenuItem(this._fans);
        const light = new PopupMenu.PopupSubMenuMenuItem('Lighting');
        for (const [name, hex] of Object.entries(COLORS)) {
            light.menu.addAction(name, () => this._proxy.LightingStaticRemote(hex, ['shelf', 'alienhead'], () => {}));
        }
        light.menu.addAction('Restore firmware animation', () => this._proxy.LightingRestoreRemote(() => {}));
        this._btn.menu.addMenuItem(light);
        this._btn.menu.connect('open-state-changed', (_m, open) => { if (open) this._refresh(); });
        Main.panel.addToStatusArea('awcc-horcrux', this._btn);
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 5, () => { this._refresh(); return GLib.SOURCE_CONTINUE; });
    }

    _refresh() {
        if (!this._proxy) return;
        this._proxy.GetStatusRemote(([json], err) => {
            if (err || !json) return;
            const st = JSON.parse(json);
            this._label.text = `AW ${st.profile ?? '?'}`;
            this._profiles.menu.removeAll();
            for (const p of st.choices) {
                const it = new PopupMenu.PopupMenuItem(p);
                it.setOrnament(p === st.profile ? PopupMenu.Ornament.DOT : PopupMenu.Ornament.NONE);
                it.connect('activate', () => this._proxy.SetProfileRemote(p, () => this._refresh()));
                this._profiles.menu.addMenuItem(it);
            }
            const f = st.fans.map(x => `${x.label}: ${x.rpm ?? '?'} rpm`).join('   ');
            const t = st.temps.map(x => `${x.label} ${x.celsius ?? '?'}\u00b0C`).join('  ');
            this._fans.label.text = `${t}\n${f}`;
        });
    }

    disable() {
        if (this._timer) GLib.source_remove(this._timer);
        this._timer = null;
        if (this._sig && this._proxy) this._proxy.disconnectSignal(this._sig);
        this._btn?.destroy();
        this._btn = null;
        this._proxy = null;
    }
}
