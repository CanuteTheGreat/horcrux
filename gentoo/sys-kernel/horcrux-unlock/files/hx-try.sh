#!/bin/sh
# stdin = the passphrase typed by the user (echo is off). Exits 0 only when the swap opened.
IFS= read -r pp || exit 1
printf %s "$pp" | cryptsetup open --type luks2 --key-file=- --allow-discards "$1" hxswap >/dev/null 2>&1 || { unset pp; exit 1; }
umask 077
mkdir -p /run/hx
printf %s "$pp" > /run/hx/key
unset pp
exit 0
