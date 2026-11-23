// Decisions and wording of the Horcrux network notification, kept free of GNOME imports so it can be tested on its own
// (tests/test_network_notify.py runs tests/network_notify_policy.mjs with node or gjs).

// What to do once a failed connection has had its grace period.
//   recovered: the same device is connected again (a roam, an access point restart, a brief drop)
//   secondary: the machine is still online through another link (Wi-Fi drop while the cable is in)
//   repeat:    this device already raised a notification a short while ago, or one is still on screen
//   notify:    the machine really lost this connection and nothing else carries it
export function decide({deviceActivated, otherLinkOnline, notifyWhenOtherOnline, secondsSinceLast, repeatInterval, shownNow}) {
    if (deviceActivated)
        return 'recovered';
    if (otherLinkOnline && !notifyWhenOtherOnline)
        return 'secondary';
    if (shownNow || secondsSinceLast < repeatInterval)
        return 'repeat';
    return 'notify';
}

const REASONS = {
    'supplicant-disconnect': 'the Wi-Fi link was lost',
    'supplicant-timeout': 'the Wi-Fi network did not answer in time',
    'supplicant-failed': 'the Wi-Fi connection could not be set up',
    'supplicant-config-failed': 'the Wi-Fi settings were refused',
    'ssid-not-found': 'the network is out of range',
    'carrier': 'the network cable was unplugged',
    'ip-config-unavailable': 'the network gave no address (DHCP)',
    'ip-config-expired': 'the network address expired',
    'config-failed': 'the connection could not be set up',
    'no-secrets': 'the password was not given',
    'dependency-failed': 'a connection it depends on failed',
    'modem-no-carrier': 'the mobile network dropped the call',
    'device-disconnected': 'the device was disconnected',
};

// NetworkManager reason nick (e.g. "supplicant-disconnect") -> words for people.
export function reasonText(nick) {
    if (!nick)
        return 'the connection was lost';
    return REASONS[nick] ?? nick.replace(/-/g, ' ');
}

// Title and body that name the network and the interface (the stock text named neither).
export function describe({kind, connection, iface, reasonNick, graceSeconds, otherLinkOnline, otherConnection}) {
    const what = connection ? `${kind} “${connection}”` : `${kind} (${iface})`;
    const title = `${what} disconnected`;
    let body = `${iface}: ${reasonText(reasonNick)}, and it did not reconnect within ${graceSeconds} s.`;
    body += otherLinkOnline
        ? ` Still online through ${otherConnection || 'another connection'}.`
        : ' This computer has no other network connection.';
    return {title, body};
}

// NM device type -> the word people use for it.
export function kindName(type) {
    return {wifi: 'Wi-Fi', ethernet: 'Wired', modem: 'Mobile broadband', bt: 'Bluetooth'}[type] ?? 'Network';
}
