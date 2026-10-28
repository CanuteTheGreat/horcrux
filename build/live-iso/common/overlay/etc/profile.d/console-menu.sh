# Installed system only: root logging in on the system console (VGA tty1, serial, hypervisor console)
# lands in the console menu (assign interfaces, set IP, reset password, ... shell = option 8).
# Other terminals (tty2-6, ssh) get a plain shell. `touch /etc/live-installer/no-console-menu` turns it off.
case $- in *i*) _cm_i=1 ;; *) _cm_i= ;; esac
if [ -n "$_cm_i" ] && [ -z "$CONSOLE_MENU_INNER" ] && [ "$(id -u)" = 0 ] \
   && [ -e /etc/live-installer/installed ] && [ ! -e /etc/live-installer/no-console-menu ]; then
	case "$(tty 2>/dev/null)" in
		/dev/tty1|/dev/ttyS0|/dev/hvc0) CONSOLE_MENU_INNER=1; export CONSOLE_MENU_INNER; /usr/bin/console-menu; exit 0 ;;
	esac
fi
