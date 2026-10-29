# show the hogshead status on the console logins (tty1 / serial), not on ssh sessions
case "$(tty 2>/dev/null)" in
  /dev/tty[0-9]*|/dev/ttyS[0-9]*|/dev/console) [ -z "$HOGSHEAD_MOTD_SHOWN" ] && [ -z "$SSH_CONNECTION" ] && { export HOGSHEAD_MOTD_SHOWN=1; /usr/bin/hogshead-motd; } ;;
esac
