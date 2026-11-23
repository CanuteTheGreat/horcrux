// Horcrux network notifications for GNOME Shell.
//
// Stock GNOME Shell (status/network.js) posts "Connection failed / Activation of network connection failed" the moment ANY
// NetworkManager device enters the FAILED state: it names neither the network nor the interface, and it fires for a Wi-Fi
// roam that fails once and reconnects a second later, for an access point that restarts, and for the Wi-Fi of a machine that
// is online through its cable the whole time. This extension replaces that notification:
//   - a failed device gets failure-grace-seconds (default 30) to come back; if it does, nothing is shown;
//   - while another connection carries the machine (NetworkManager's primary connection is active on another device) nothing
//     is shown either, unless notify-when-other-link-online is set;
//   - otherwise ONE notification names the connection, the interface and the reason, and goes away when the device is back;
//   - one device notifies at most once per repeat-interval-seconds, so a flapping link never piles notifications up.
// Settings: org.gnome.shell.extensions.horcrux-network-notify (gsettings / dconf; system defaults in /etc/dconf/db/local.d).

import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import NM from 'gi://NM';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as MessageTray from 'resource:///org/gnome/shell/ui/messageTray.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {decide, describe, kindName} from './policy.js';

const LOG = 'horcrux-network-notify:';
const HOOK_TRIES = 120; // x 500 ms: the quick settings load their indicators asynchronously

const TYPES = new Map([
    [NM.DeviceType.WIFI, 'wifi'],
    [NM.DeviceType.ETHERNET, 'ethernet'],
    [NM.DeviceType.MODEM, 'modem'],
    [NM.DeviceType.BT, 'bt'],
]);

function reasonNick(reason) {
    for (const [k, v] of Object.entries(NM.DeviceStateReason)) {
        if (v === reason)
            return k.toLowerCase().replace(/_/g, '-');
    }
    return `${reason}`;
}

export default class HorcruxNetworkNotify extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._gen = (this._gen ?? 0) + 1;
        this._devices = new Map();   // NM.Device -> signal id
        this._names = new Map();     // device path -> last connection name
        this._pending = new Map();   // device path -> GLib source id
        this._shown = new Map();     // device path -> notification
        this._lastShown = new Map(); // device path -> monotonic time (us)
        this._hookTries = 0;
        this._hookIndicator();

        const gen = this._gen;
        NM.Client.new_async(null, (_o, res) => {
            let client;
            try {
                client = NM.Client.new_finish(res);
            } catch (e) {
                console.warn(`${LOG} no NetworkManager client: ${e.message}`);
                return;
            }
            if (gen !== this._gen || !this._settings)
                return; // disabled meanwhile
            this._client = client;
            this._clientIds = [
                client.connect('device-added', (_c, d) => this._watch(d)),
                client.connect('device-removed', (_c, d) => this._unwatch(d)),
            ];
            for (const d of client.get_devices())
                this._watch(d);
            console.log(`${LOG} watching ${this._devices.size} devices`);
        });
    }

    disable() {
        this._gen++;
        if (this._hookId)
            GLib.source_remove(this._hookId);
        this._hookId = 0;
        if (this._indicator)
            delete this._indicator._onActivationFailed; // the stock method again
        this._indicator = null;
        for (const id of this._pending.values())
            GLib.source_remove(id);
        this._pending.clear();
        for (const d of [...this._devices.keys()])
            this._unwatch(d);
        if (this._client)
            this._clientIds.forEach(id => GObject.signal_handler_disconnect(this._client, id));
        this._client = null;
        for (const n of this._shown.values())
            n.destroy(MessageTray.NotificationDestroyedReason.SOURCE_CLOSED);
        this._shown.clear();
        this._settings = null;
    }

    // Replace the stock "Connection failed" notification of the network indicator (instance property: disable() deletes it).
    _hookIndicator() {
        const ind = Main.panel.statusArea.quickSettings?._network;
        if (ind) {
            this._indicator = ind;
            ind._onActivationFailed = () => {};
            console.log(`${LOG} stock connection-failed notification replaced`);
            return;
        }
        if (this._hookTries++ >= HOOK_TRIES) {
            console.warn(`${LOG} network indicator not found, stock notification left in place`);
            return;
        }
        this._hookId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 500, () => {
            this._hookId = 0;
            this._hookIndicator();
            return GLib.SOURCE_REMOVE;
        });
    }

    _watch(d) {
        if (this._devices.has(d))
            return;
        this._devices.set(d, d.connect('state-changed', (dev, n, o, r) => this._stateChanged(dev, n, o, r)));
        this._remember(d);
    }

    _unwatch(d) {
        const id = this._devices.get(d);
        if (id)
            // NOT d.disconnect(id): NM.Device has its own disconnect(cancellable, callback) method (disconnect the device), which shadows
            // GObject's and throws "Expected an object of type GCancellable" here, aborting disable() and every device-removed signal
            GObject.signal_handler_disconnect(d, id);
        this._devices.delete(d);
        this._cancel(d.get_path());
    }

    _remember(d) {
        const ac = d.get_active_connection();
        if (ac?.get_id())
            this._names.set(d.get_path(), ac.get_id());
    }

    _cancel(path) {
        const id = this._pending.get(path);
        if (id)
            GLib.source_remove(id);
        this._pending.delete(path);
    }

    _stateChanged(d, newState, _oldState, reason) {
        const path = d.get_path();
        if (newState === NM.DeviceState.ACTIVATED) {
            this._cancel(path);
            this._remember(d);
            this._shown.get(path)?.destroy();
            return;
        }
        if (newState >= NM.DeviceState.PREPARE && newState < NM.DeviceState.ACTIVATED)
            this._remember(d);
        // as stock: no secrets = the user cancelled the password dialog
        if (newState !== NM.DeviceState.FAILED || reason === NM.DeviceStateReason.NO_SECRETS)
            return;
        const grace = this._settings.get_uint('failure-grace-seconds');
        this._cancel(path);
        console.log(`${LOG} ${d.get_iface()} (${this._names.get(path) ?? '?'}) failed: ${reasonNick(reason)}; judged in ${grace} s`);
        this._pending.set(path, GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, Math.max(grace, 1), () => {
            this._pending.delete(path);
            this._judge(d, reason, grace);
            return GLib.SOURCE_REMOVE;
        }));
    }

    _judge(d, reason, grace) {
        if (!this._settings || !this._devices.has(d))
            return;
        const path = d.get_path();
        const primary = this._client?.get_primary_connection();
        const otherLinkOnline = !!primary &&
            primary.get_state() === NM.ActiveConnectionState.ACTIVATED &&
            !(primary.get_devices() ?? []).includes(d) &&
            this._client.get_connectivity() !== NM.ConnectivityState.NONE;
        const now = GLib.get_monotonic_time();
        const last = this._lastShown.get(path);
        const verdict = decide({
            deviceActivated: d.get_state() === NM.DeviceState.ACTIVATED,
            otherLinkOnline,
            notifyWhenOtherOnline: this._settings.get_boolean('notify-when-other-link-online'),
            secondsSinceLast: last === undefined ? Infinity : (now - last) / 1e6,
            repeatInterval: this._settings.get_uint('repeat-interval-seconds'),
            shownNow: this._shown.has(path),
        });
        console.log(`${LOG} ${d.get_iface()}: ${verdict}`);
        if (verdict !== 'notify')
            return;
        const {title, body} = describe({
            kind: kindName(TYPES.get(d.get_device_type())),
            connection: this._names.get(path),
            iface: d.get_iface(),
            reasonNick: reasonNick(reason),
            graceSeconds: grace,
            otherLinkOnline,
            otherConnection: primary?.get_id(),
        });
        const source = MessageTray.getSystemSource();
        const n = new MessageTray.Notification({
            source,
            title,
            body,
            iconName: 'network-error-symbolic',
            urgency: MessageTray.Urgency.HIGH,
            isTransient: true,
        });
        n.connect('destroy', () => {
            if (this._shown.get(path) === n)
                this._shown.delete(path);
        });
        this._shown.set(path, n);
        this._lastShown.set(path, now);
        console.log(`${LOG} notify: ${title}: ${body}`);
        source.addNotification(n);
    }
}
