#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2024-2026 NitroSense Contributors
set -eu

if [ "${DESTDIR:-}" = "" ] && [ "$(id -u)" -ne 0 ]; then
    echo "uninstall.sh must be run as root (use sudo ./uninstall.sh)" >&2
    exit 1
fi

destdir=${DESTDIR:-}

rm -f \
    "${destdir}/usr/bin/nitrosense" \
    "${destdir}/usr/bin/nitro-sense" \
    "${destdir}/usr/share/applications/nitrosense.desktop" \
    "${destdir}/usr/share/icons/hicolor/256x256/apps/nitrosense.png" \
    "${destdir}/usr/share/polkit-1/actions/com.nitrosense.launch.policy"

rm -rf "${destdir}/usr/local/share/fonts/nitrosense"

config_dir="${destdir}/etc/nitrosense"
if [ -d "$config_dir" ]; then
    printf "Remove %s and all saved NitroSense settings? [y/N] " "$config_dir"
    if ! read answer; then
        answer=
    fi
    case "$answer" in
        y|Y|yes|YES)
            rm -rf "$config_dir"
            ;;
        *)
            echo "Keeping $config_dir"
            ;;
    esac
fi

if [ "${DESTDIR:-}" != "" ]; then
    echo "Skipping fc-cache for DESTDIR staging."
elif command -v fc-cache >/dev/null 2>&1; then
    fc-cache -f
else
    echo "warning: fc-cache not found; font cache was not refreshed" >&2
fi

echo "NitroSense uninstalled successfully."
