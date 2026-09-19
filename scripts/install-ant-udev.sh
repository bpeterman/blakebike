#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
rule_source="${script_dir}/../resources/99-blakebike-ant.rules"
rule_target="/etc/udev/rules.d/99-blakebike-ant.rules"

if [[ ! -f "${rule_source}" ]]; then
  echo "ANT udev rule not found: ${rule_source}" >&2
  exit 1
fi

sudo install -m 0644 "${rule_source}" "${rule_target}"
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=usb --subsystem-match=tty

echo "Installed ${rule_target}"
echo "Unplug and reconnect the ANT USB stick, then restart BlakeBike."
