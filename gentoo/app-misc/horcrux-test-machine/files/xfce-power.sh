#!/bin/sh
# app-misc/horcrux-test-machine[xfce]: run at every XFCE login: a TEST machine never sleeps on lid / idle and never blanks the screen.
# xfce4-power-manager's own defaults are in a file owned by horcrux-surface-desktop (lid=suspend), per-user values win, so set them per user, idempotently.
command -v xfconf-query >/dev/null 2>&1 || exit 0
q() { xfconf-query -c xfce4-power-manager -p "/xfce4-power-manager/$1" -n -t "$2" -s "$3" 2>/dev/null || xfconf-query -c xfce4-power-manager -p "/xfce4-power-manager/$1" -s "$3"; }
for p in lid-action-on-ac lid-action-on-battery sleep-button-action hibernate-button-action; do q $p uint 0; done
q power-button-action uint 3
for p in inactivity-on-ac inactivity-on-battery; do q $p uint 14; done
q blank-on-ac int 0; q blank-on-battery int 0; q dpms-enabled bool false
q lock-screen-suspend-hibernate bool false
exit 0
