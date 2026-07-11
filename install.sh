#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2024-2026 NitroSense Contributors
set -eu

if [ "${DESTDIR:-}" = "" ] && [ "$(id -u)" -ne 0 ]; then
    echo "install.sh must be run as root (use sudo ./install.sh)" >&2
    exit 1
fi

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
binary_src="${repo_dir}/target/release/nitrosense"

refuse_root_build() {
    echo "target/release/nitrosense is missing; refusing to run cargo build as root" >&2
    echo "Run 'cargo build --release' as an unprivileged user, then rerun install.sh." >&2
    exit 1
}

build_as_current_user() {
    if ! command -v cargo >/dev/null 2>&1; then
        echo "target/release/nitrosense is missing and cargo was not found" >&2
        exit 1
    fi
    cargo build --release --manifest-path "${repo_dir}/Cargo.toml"
}

build_as_sudo_user() {
    if ! command -v sudo >/dev/null 2>&1; then
        refuse_root_build
    fi

    if ! sudo -H -u "$SUDO_USER" sh -c \
        'command -v cargo >/dev/null 2>&1 || [ -x "$HOME/.cargo/bin/cargo" ]'; then
        echo "target/release/nitrosense is missing and cargo was not found for ${SUDO_USER}" >&2
        exit 1
    fi

    sudo -H -u "$SUDO_USER" sh -c '
        if command -v cargo >/dev/null 2>&1; then
            exec cargo build --release --manifest-path "$1"
        fi
        exec "$HOME/.cargo/bin/cargo" build --release --manifest-path "$1"
    ' sh "${repo_dir}/Cargo.toml"
}

if [ ! -x "$binary_src" ]; then
    if [ "$(id -u)" -eq 0 ]; then
        if [ "${SUDO_USER:-}" != "" ] && [ "${SUDO_USER:-}" != "root" ]; then
            build_as_sudo_user
        else
            refuse_root_build
        fi
    else
        build_as_current_user
    fi
fi

if [ ! -x "$binary_src" ]; then
    echo "release build did not produce $binary_src" >&2
    exit 1
fi

destdir=${DESTDIR:-}
install -d -m 0755 \
    "${destdir}/usr/bin" \
    "${destdir}/usr/share/applications" \
    "${destdir}/usr/share/icons/hicolor/256x256/apps" \
    "${destdir}/usr/share/polkit-1/actions" \
    "${destdir}/usr/local/share/fonts/nitrosense" \
    "${destdir}/etc/nitrosense"

install -m 0755 "$binary_src" "${destdir}/usr/bin/nitrosense"

cat > "${destdir}/usr/bin/nitro-sense" <<'EOF'
#!/bin/sh
# Preserve the caller's Wayland/X11 session for pkexec elevation (Hyprland, etc.).
exec pkexec env \
    DISPLAY="${DISPLAY-}" \
    WAYLAND_DISPLAY="${WAYLAND_DISPLAY-}" \
    XAUTHORITY="${XAUTHORITY-}" \
    XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR-}" \
    DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS-}" \
    /usr/bin/nitrosense "$@"
EOF
chmod 0755 "${destdir}/usr/bin/nitro-sense"

install -m 0644 "${repo_dir}/assets/nitrosense.desktop" \
    "${destdir}/usr/share/applications/nitrosense.desktop"
install -m 0644 "${repo_dir}/assets/com.nitrosense.launch.policy" \
    "${destdir}/usr/share/polkit-1/actions/com.nitrosense.launch.policy"
install -m 0644 "${repo_dir}/assets/nitrosense.png" \
    "${destdir}/usr/share/icons/hicolor/256x256/apps/nitrosense.png"

for font in "${repo_dir}"/assets/fonts/Squares*.otf; do
    [ -f "$font" ] || continue
    install -m 0644 "$font" "${destdir}/usr/local/share/fonts/nitrosense/"
done

chmod 0755 "${destdir}/etc/nitrosense"

if [ "${DESTDIR:-}" != "" ]; then
    echo "Skipping fc-cache for DESTDIR staging."
elif command -v fc-cache >/dev/null 2>&1; then
    fc-cache -f
else
    echo "warning: fc-cache not found; font cache was not refreshed" >&2
fi

echo "NitroSense installed successfully."

if [ "${DESTDIR:-}" = "" ]; then
    rgb_ready=0
    if [ -e /dev/acer-gkbbl-0 ] && [ -e /dev/acer-gkbbl-static-0 ]; then
        rgb_ready=1
    elif [ -e /sys/devices/platform/acer-wmi/four_zoned_kb/four_zone_mode ] \
        && [ -e /sys/devices/platform/acer-wmi/four_zoned_kb/per_zone_mode ]; then
        rgb_ready=1
    fi

    if [ "$rgb_ready" -eq 0 ]; then
        echo "warning: RGB keyboard interface not detected." >&2
        if [ -d /sys/module/linuwu_sense ]; then
            echo "warning: linuwu_sense is loaded but four_zoned_kb is missing for this model." >&2
            echo "warning: For Nitro AN515-4x install acer-predator-turbo-and-rgb-keyboard-linux-module" >&2
            echo "warning: (unload linuwu_sense first) or enable your model in Linuwu-Sense." >&2
        elif ls /sys/bus/wmi/devices/7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56-* >/dev/null 2>&1; then
            echo "warning: Install acer-predator-turbo-and-rgb-keyboard-linux-module or Linuwu-Sense." >&2
        fi
    fi
fi
