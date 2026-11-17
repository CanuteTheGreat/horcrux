# User-journey review checklist (LLM vision pass over `user_journey.py` screenshots)

You are a competent Linux/virtualisation sysadmin who has never seen Horcrux, doing a first install on a fresh box.
Look at each screenshot in journey order (`NN-name.png`) and the scripted results (`journey.json`: steps, findings,
console errors). For EVERY screen ask:

1. **Confusing** - could I tell what this does and what to do next without reading source or docs? Jargon, unlabelled
   fields, units missing, two controls that look the same, a button whose result I cannot predict.
2. **Missing** - what would I expect here that is absent? (A sensible default, a way to add the thing I need right
   here - not "go to another page first" - a slider/preset for a bounded value, a custom override next to a preset,
   an upload next to a download, a status/next step after an action.)
3. **Redundant / pointless** - a button that repeats the text next to it, a control that does nothing in this state, a
   "Download" button inside the empty state of a page whose form is already above it, two routes to the same thing.
4. **Wrong default** - a new VM with no NIC, no way to make a disk bootable, CD-ROM list that cannot contain the ISO I
   just added, sound unavailable for the most common console, a form that only fails at Submit with a raw 400.
5. **Dead / lying** - empty state that says "none" while something exists, numbers that are NaN/0 in red, a banner
   that is wrong, a link that goes nowhere, a spinner that never ends, an action that silently does nothing.
6. **Operational** - does the product tell me the host is out of date (updates pending, reboot needed)? Does a fresh
   install give me a usable network (bridge/NAT), storage pool, ISO source? Can I get from "empty box" to "VM booting
   an installer" without leaving the UI?

Classes the owner already found by hand (any new instance is a finding, not a judgement call):
- create-VM form: CPU/RAM/disk as bare number boxes (no sliders, no supported steps/presets, no custom override)
- a new VM has no network interface by default; the form fails at Create when the host has no bridge
- no sound device can be added when the display is VNC
- CD-ROM/ISO list offers only "(empty drive)"; an ISO cannot be added from the form
- ISO Library: empty-state text pointing at a button above plus a redundant button; no upload
- nothing shows that the node never installed updates

Output: one finding per issue: `journey:<step>:<short-slug> | screen NN-name | what a sysadmin sees | what it should be`.
Fix with a test where the issue is code (add/extend the journey assertion FIRST so it fails, then fix, then remove it from
`ci/ux-qa/user-journey-baseline.json`); never ask the owner; judgement calls that really need a decision go to the daily
summary only.
