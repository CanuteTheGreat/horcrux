# Start the Surface desktop on the first virtual console for a normal (non-root) user.
# Opt out per user: touch ~/.no-desktop ; or log in on tty2-6 for a plain shell.
if [ "$(id -u)" != 0 ] && [ -z "$WAYLAND_DISPLAY$DISPLAY$SSH_CONNECTION" ] && [ "$(tty 2>/dev/null)" = /dev/tty1 ] \
   && [ ! -e "$HOME/.no-desktop" ] && [ -x /usr/libexec/surface-session ]; then
    exec /usr/libexec/surface-session
fi
