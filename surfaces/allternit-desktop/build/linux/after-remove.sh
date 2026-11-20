#!/bin/bash
# Allternit Desktop afterRemove script, shared by the .deb (postrm) and the
# .rpm (%postun). package.json points deb.afterRemove and rpm.afterRemove here.
#
# Based on electron-builder's stock after-remove.tpl, with one change: it does
# nothing during an upgrade. rpm runs the OLD package's %postun after the NEW
# package's %post, so the stock script removed the /usr/bin/allternit link and
# the AppArmor profile that the new version had just installed (every rpm
# update, the in-app RpmUpdater included, left the app without its command).
#   dpkg: $1 is "remove", "purge", "upgrade", "failed-upgrade", ...
#   rpm:  $1 is the number of copies left (0 = erase, 1+ = upgrade)

case "${1:-}" in
    upgrade|failed-upgrade|abort-install|abort-upgrade|disappear) exit 0 ;;
    [1-9]*) exit 0 ;;
esac

APP_DIR='/opt/Allternit Desktop'

# update-alternatives --remove <name> <path>: 'path' is the registered
# alternative binary, not the generic symlink.
if type update-alternatives >/dev/null 2>&1; then
    update-alternatives --remove 'allternit' "$APP_DIR/allternit" || true
fi
if [ -L '/usr/bin/allternit' ] && [ ! -e '/usr/bin/allternit' ]; then
    rm -f '/usr/bin/allternit'
fi

APPARMOR_PROFILE_DEST='/etc/apparmor.d/allternit'

# Remove and unload the AppArmor profile (Ubuntu 24+, openSUSE Leap).
if [ -f "$APPARMOR_PROFILE_DEST" ]; then
  if apparmor_status --enabled > /dev/null 2>&1; then
    if ! { [ -x '/usr/bin/ischroot' ] && /usr/bin/ischroot; } && hash apparmor_parser 2>/dev/null; then
      apparmor_parser --remove "$APPARMOR_PROFILE_DEST" || true
    fi
  fi
  rm -f "$APPARMOR_PROFILE_DEST"
fi

exit 0
