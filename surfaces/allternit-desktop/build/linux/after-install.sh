#!/bin/bash
# Allternit Desktop .deb afterInstall (postinst) script.
#
# Based on electron-builder's stock after-install.tpl, with one change: the
# user-namespace test runs as an UNPRIVILEGED user. The stock template runs
# `unshare --user true` as root (dpkg runs postinst as root), and root can
# always create a user namespace, so the test always passed and chrome-sandbox
# stayed 0755. On machines where normal users cannot create user namespaces
# (Debian with kernel.unprivileged_userns_clone=0, hardened kernels, Ubuntu
# 24.04 without the AppArmor profile) the app then died at launch with
# "FATAL credentials.cc:131". Now those machines get the setuid sandbox.

APP_DIR='/opt/Allternit Desktop'
SANDBOX="$APP_DIR/chrome-sandbox"

if type update-alternatives >/dev/null 2>&1; then
    # Remove previous link if it doesn't use update-alternatives
    if [ -L '/usr/bin/allternit' ] && [ -e '/usr/bin/allternit' ] && [ "$(readlink '/usr/bin/allternit')" != '/etc/alternatives/allternit' ]; then
        rm -f '/usr/bin/allternit'
    fi
    update-alternatives --install '/usr/bin/allternit' 'allternit' "$APP_DIR/allternit" 100 || ln -sf "$APP_DIR/allternit" '/usr/bin/allternit'
else
    ln -sf "$APP_DIR/allternit" '/usr/bin/allternit'
fi

# Can an unprivileged user create a user namespace? Returns 0 only when a
# real unprivileged test succeeded. Anything we cannot test counts as "no",
# which is the safe answer (the setuid sandbox works either way).
userns_works_for_normal_users() {
    [ -L /proc/self/ns/user ] || return 1
    command -v unshare >/dev/null 2>&1 || return 1
    if command -v setpriv >/dev/null 2>&1; then
        # setpriv (util-linux) needs no PAM or login shell; 65534 is nobody.
        setpriv --reuid=65534 --regid=65534 --clear-groups unshare --user true >/dev/null 2>&1
        return $?
    fi
    if command -v su >/dev/null 2>&1; then
        su nobody -s /bin/sh -c 'unshare --user true' >/dev/null 2>&1
        return $?
    fi
    return 1
}

if [ -e "$SANDBOX" ]; then
    if userns_works_for_normal_users; then
        chmod 0755 "$SANDBOX" || true
    else
        # No usable user namespaces for normal users: use the SUID sandbox.
        chown root:root "$SANDBOX" || true
        chmod 4755 "$SANDBOX" || true
    fi
fi

if hash update-mime-database 2>/dev/null; then
    update-mime-database /usr/share/mime || true
fi

if hash update-desktop-database 2>/dev/null; then
    update-desktop-database /usr/share/applications || true
fi

# Install apparmor profile. (Ubuntu 24+)
# First check if the version of AppArmor running on the device supports our profile.
# This is in order to keep backwards compatibility with Ubuntu 22.04 which does not support abi/4.0.
# In that case, we just skip installing the profile since the app runs fine without it on 22.04.
#
# Those apparmor_parser flags are akin to performing a dry run of loading a profile.
# https://wiki.debian.org/AppArmor/HowToUse#Dumping_profiles
#
# Unfortunately, at the moment AppArmor doesn't have a good story for backwards compatibility.
# https://askubuntu.com/questions/1517272/writing-a-backwards-compatible-apparmor-profile
if apparmor_status --enabled > /dev/null 2>&1; then
  APPARMOR_PROFILE_SOURCE="$APP_DIR/resources/apparmor-profile"
  APPARMOR_PROFILE_TARGET='/etc/apparmor.d/allternit'
  if apparmor_parser --skip-kernel-load --debug "$APPARMOR_PROFILE_SOURCE" > /dev/null 2>&1; then
    cp -f "$APPARMOR_PROFILE_SOURCE" "$APPARMOR_PROFILE_TARGET"

    # Updating the current AppArmor profile is not possible and probably not meaningful in a chroot'ed environment.
    # Use cases are for example environments where images for clients are maintained.
    # There, AppArmor might correctly be installed, but live updating makes no sense.
    if ! { [ -x '/usr/bin/ischroot' ] && /usr/bin/ischroot; } && hash apparmor_parser 2>/dev/null; then
      # Extra flags taken from dh_apparmor:
      # > By using '-W -T' we ensure that any abstraction updates are also pulled in.
      # https://wiki.debian.org/AppArmor/Contribute/FirstTimeProfileImport
      apparmor_parser --replace --write-cache --skip-read-cache "$APPARMOR_PROFILE_TARGET"
    fi
  else
    echo "Skipping the installation of the AppArmor profile as this version of AppArmor does not seem to support the bundled profile"
  fi
fi

exit 0
