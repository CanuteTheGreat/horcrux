// node build/live-iso/common/tests/network_notify_policy.mjs <policy.js>   (or gjs -m): exits 1 on the first wrong answer
import {pathToFileURL} from 'node:url';

const path = process.argv[2];
const {decide, describe, reasonText, kindName} = await import(pathToFileURL(path).href);

let fails = 0;
function eq(name, got, want) {
    if (JSON.stringify(got) !== JSON.stringify(want)) {
        console.log(`FAIL ${name}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
        fails++;
    } else {
        console.log(`ok   ${name}`);
    }
}
const base = {deviceActivated: false, otherLinkOnline: false, notifyWhenOtherOnline: false, secondsSinceLast: Infinity,
    repeatInterval: 900, shownNow: false};

// a roam that failed once and reconnected within the grace period
eq('roam-recovered', decide({...base, deviceActivated: true}), 'recovered');
// Wi-Fi dropped while the cable keeps the machine online
eq('wifi-drop-while-wired', decide({...base, otherLinkOnline: true}), 'secondary');
eq('wifi-drop-while-wired-opt-in', decide({...base, otherLinkOnline: true, notifyWhenOtherOnline: true}), 'notify');
// the machine really is offline
eq('really-offline', decide(base), 'notify');
// a flapping link (access point restarts at night) does not pile notifications up
eq('flap-within-interval', decide({...base, secondsSinceLast: 120}), 'repeat');
eq('flap-still-on-screen', decide({...base, shownNow: true}), 'repeat');
eq('flap-after-interval', decide({...base, secondsSinceLast: 901}), 'notify');
// recovery wins over everything
eq('recovered-beats-repeat', decide({...base, deviceActivated: true, shownNow: true}), 'recovered');

const d = describe({kind: kindName('wifi'), connection: 'Home Net', iface: 'wlan0', reasonNick: 'supplicant-disconnect',
    graceSeconds: 30, otherLinkOnline: false});
eq('title-names-network', d.title, 'Wi-Fi “Home Net” disconnected');
eq('body-names-interface-and-reason', d.body.startsWith('wlan0: the Wi-Fi link was lost, and it did not reconnect within 30 s.'), true);
eq('body-says-offline', d.body.endsWith('This computer has no other network connection.'), true);
const w = describe({kind: kindName('ethernet'), connection: '', iface: 'enp1s0', reasonNick: 'carrier', graceSeconds: 30,
    otherLinkOnline: true, otherConnection: 'Home Net'});
eq('title-without-connection-names-iface', w.title, 'Wired (enp1s0) disconnected');
eq('body-names-other-link', w.body.endsWith('Still online through Home Net.'), true);
eq('unknown-reason-readable', reasonText('some-new-reason'), 'some new reason');
eq('kind-unknown', kindName('vlan'), 'Network');

if (fails) {
    console.log(`${fails} failed`);
    process.exit(1);
}
console.log('PASS');
