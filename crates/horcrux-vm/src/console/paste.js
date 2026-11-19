// Console "Paste": type text into the guest as key events.
//
// The VNC server only hands a clipboard (RFB ClientCutText) to a guest that
// runs a clipboard agent; BIOS menus, boot loaders, installers and disk-unlock
// prompts have none, so a paste there was dropped silently. Typing needs
// nothing from the guest: every character becomes a key press/release the
// guest sees as keyboard input.
//
// Included verbatim in the noVNC page (get_novnc_html) and exercised in a real
// browser by horcrux-ui/e2e/console_paste_e2e.py. Never logs the text.
const HorcruxPaste = (() => {
    const SHIFT_L = 0xffe1;
    const RETURN = 0xff0d;
    const TAB = 0xff09;
    // Symbols that need Shift on a US keyboard. QEMU's VNC server maps a
    // keysym to the key that produces it in the VM's keyboard layout but does
    // not add the Shift itself, so the client presses it (as every VNC client
    // does). Other layouts: Shift is added for A-Z only; the VM's
    // `keyboard_layout` makes QEMU pick the right keys for the rest.
    const US_SHIFTED = '~!@#$%^&*()_+{}|:"<>?';

    function usesUsShiftTable(layout) {
        return !layout || String(layout).toLowerCase() === 'en-us';
    }

    // One step per character: {keysym, shift, newline}. `skipped` counts
    // characters that have no key (control characters); `nonAscii` counts the
    // ones the VNC server can only type if the VM's layout has them.
    function plan(text, layout, opts) {
        const unicode = !opts || opts.unicode !== false;
        const us = usesUsShiftTable(layout);
        const steps = [];
        let skipped = 0;
        let nonAscii = 0;
        const norm = String(text).replace(/\r\n?/g, '\n');
        for (const ch of norm) {
            const cp = ch.codePointAt(0);
            if (ch === '\n') {
                steps.push({ keysym: RETURN, shift: false, newline: true });
            } else if (ch === '\t') {
                steps.push({ keysym: TAB, shift: false, newline: false });
            } else if (cp < 0x20 || (cp >= 0x7f && cp < 0xa0)) {
                skipped++;
            } else if (cp < 0x7f) {
                const upper = cp >= 0x41 && cp <= 0x5a;
                const shifted = upper || (us && US_SHIFTED.includes(ch));
                steps.push({ keysym: cp, shift: shifted, newline: false });
            } else if (!unicode) {
                nonAscii++;
                skipped++;
            } else {
                nonAscii++;
                // Latin-1 keysyms equal their code point; the rest use the
                // X11 Unicode keysym range.
                steps.push({
                    keysym: cp <= 0xff ? cp : 0x01000000 + cp,
                    shift: false,
                    newline: false,
                });
            }
        }
        return { steps, skipped, nonAscii };
    }

    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

    // Types the planned steps. `job.cancelled` stops it between keys.
    // `o`: {delay, lineDelay, onProgress(done,total)}. Releases Shift on any
    // exit so a cancelled paste never leaves a modifier stuck in the guest.
    async function type(rfb, steps, job, o) {
        let done = 0;
        let shiftDown = false;
        try {
            for (const s of steps) {
                if (job.cancelled) break;
                // A guest that drops the Shift press or release garbles the
                // rest of the text (stuck Shift): give each change its own tick.
                if (s.shift && !shiftDown) {
                    rfb.sendKey(SHIFT_L, 'ShiftLeft', true);
                    shiftDown = true;
                    if (o.delay > 0) await sleep(o.delay);
                } else if (!s.shift && shiftDown) {
                    rfb.sendKey(SHIFT_L, 'ShiftLeft', false);
                    shiftDown = false;
                    if (o.delay > 0) await sleep(o.delay);
                }
                rfb.sendKey(s.keysym, null, true);
                rfb.sendKey(s.keysym, null, false);
                done++;
                if (o.onProgress && (done % 10 === 0 || done === steps.length)) {
                    o.onProgress(done, steps.length);
                }
                const d = s.newline ? o.lineDelay : o.delay;
                if (d > 0) await sleep(d);
            }
        } finally {
            if (shiftDown) rfb.sendKey(SHIFT_L, 'ShiftLeft', false);
        }
        return done;
    }

    return { plan, type, SHIFT_L, RETURN, TAB };
})();
if (typeof window !== 'undefined') window.HorcruxPaste = HorcruxPaste;
if (typeof module !== 'undefined') module.exports = HorcruxPaste;
