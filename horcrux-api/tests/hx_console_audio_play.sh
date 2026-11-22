#!/bin/sh
# Guest side of the console e2e's audio check (copied onto the boot CD
# next to the alsa-utils packages). No argument: sound driver, packages,
# mixer, prints HXAUDIO_READY. `tone`: a 440 Hz tone for about 6 s.
d=$(dirname "$0")
if [ "$1" = tone ]; then
  speaker-test -D plughw:0,0 -c 2 -r 48000 -t sine -f 440 -l 3 >/dev/null 2>&1
  echo "HXAUDIO_PLAYED rc=$?"
  exit 0
fi
modprobe snd-hda-intel
apk add -q --allow-untrusted --force-non-repository --no-network "$d"/*.apk
for i in 1 2 3 4 5; do [ -e /dev/snd/pcmC0D0p ] && break; sleep 1; done
for c in Master PCM Front; do amixer -q -c 0 sset "$c" 100% unmute 2>/dev/null; done
ls /dev/snd
echo HXAUDIO_READY
