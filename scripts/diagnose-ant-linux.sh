#!/usr/bin/env bash
# Collect read-only diagnostics for BlakeBike ANT USBStick2 discovery.
# The output is intended to be safe and compact enough to paste into a bug report.

set -uo pipefail
shopt -s nullglob

readonly ant_vid="0fcf"
readonly ant_pid="1008"
readonly diagnostic_version="1"

section() {
  printf '\n===== %s =====\n' "$1"
}

attribute() {
  local path="$1"
  local label="$2"
  if [[ -r "${path}" ]]; then
    printf '%-22s %s\n' "${label}:" "$(<"${path}")"
  fi
}

show_udev_properties() {
  local sys_path="$1"
  if command -v udevadm >/dev/null 2>&1; then
    udevadm info --query=property --path="${sys_path}" 2>&1 \
      | grep -E '^(DEVNAME|DEVPATH|DEVTYPE|DRIVER|ID_BUS|ID_MODEL|ID_MODEL_ID|ID_PATH|ID_SERIAL|ID_USB_DRIVER|ID_VENDOR|ID_VENDOR_ID|MAJOR|MINOR|SUBSYSTEM|TAGS|USEC_INITIALIZED)=' \
      | sort || true
  else
    echo "udevadm is not installed"
  fi
}

show_holders() {
  local node="$1"
  if command -v fuser >/dev/null 2>&1; then
    fuser -v "${node}" 2>&1 || echo "No process reported by fuser"
  elif command -v lsof >/dev/null 2>&1; then
    lsof "${node}" 2>&1 || echo "No process reported by lsof"
  else
    echo "Neither fuser nor lsof is installed"
  fi
}

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "This diagnostic is for Linux. Detected: $(uname -s)"
  exit 2
fi

section "BlakeBike ANT diagnostics v${diagnostic_version}"
printf 'timestamp_utc: %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
printf 'kernel: %s\n' "$(uname -srmo)"
printf 'architecture: %s\n' "$(uname -m)"
printf 'user_id: %s\n' "$(id -u)"
printf 'groups: %s\n' "$(id -Gn)"
if [[ -r /etc/os-release ]]; then
  # Only emit stable distribution metadata, not the machine hostname.
  grep -E '^(NAME|VERSION|ID|ID_LIKE|VERSION_ID)=' /etc/os-release || true
fi

section "Required commands"
for command_name in lsusb udevadm fuser lsof journalctl dmesg; do
  if command -v "${command_name}" >/dev/null 2>&1; then
    printf '%-12s %s\n' "${command_name}" "$(command -v "${command_name}")"
  else
    printf '%-12s missing\n' "${command_name}"
  fi
done

section "USB enumeration"
if command -v lsusb >/dev/null 2>&1; then
  lsusb -d "${ant_vid}:${ant_pid}" 2>&1 || echo "lsusb did not find ${ant_vid}:${ant_pid}"
  echo "-- descriptors --"
  lsusb -v -d "${ant_vid}:${ant_pid}" 2>&1 || true
else
  echo "lsusb is unavailable; install the usbutils package for descriptor output"
fi

usb_devices=()
for vendor_file in /sys/bus/usb/devices/*/idVendor; do
  [[ -r "${vendor_file}" ]] || continue
  usb_device="${vendor_file%/idVendor}"
  [[ "$(<"${vendor_file}")" == "${ant_vid}" ]] || continue
  [[ -r "${usb_device}/idProduct" ]] || continue
  [[ "$(<"${usb_device}/idProduct")" == "${ant_pid}" ]] || continue
  usb_devices+=("${usb_device}")
done

tty_nodes=()
raw_usb_nodes=()

section "sysfs USB device and interfaces"
if ((${#usb_devices[@]} == 0)); then
  echo "No ${ant_vid}:${ant_pid} device exists under /sys/bus/usb/devices"
else
  for usb_device in "${usb_devices[@]}"; do
    echo "device: ${usb_device}"
    usb_device_real="$(readlink -f "${usb_device}" 2>/dev/null || printf '%s' "${usb_device}")"
    echo "device_real_path: ${usb_device_real}"
    attribute "${usb_device}/manufacturer" "manufacturer"
    attribute "${usb_device}/product" "product"
    attribute "${usb_device}/serial" "serial"
    attribute "${usb_device}/busnum" "busnum"
    attribute "${usb_device}/devnum" "devnum"
    attribute "${usb_device}/speed" "speed_mbps"
    attribute "${usb_device}/bDeviceClass" "device_class"
    attribute "${usb_device}/bNumConfigurations" "configurations"
    attribute "${usb_device}/authorized" "authorized"
    show_udev_properties "${usb_device}"

    if [[ -r "${usb_device}/busnum" && -r "${usb_device}/devnum" ]]; then
      busnum="$(<"${usb_device}/busnum")"
      devnum="$(<"${usb_device}/devnum")"
      printf -v raw_node '/dev/bus/usb/%03d/%03d' "$((10#${busnum}))" "$((10#${devnum}))"
      raw_usb_nodes+=("${raw_node}")
      if [[ -e "${raw_node}" ]]; then
        stat -Lc 'raw_usb: %A %U:%G %n' "${raw_node}" 2>&1 || true
        [[ -r "${raw_node}" ]] && raw_read="yes" || raw_read="no"
        [[ -w "${raw_node}" ]] && raw_write="yes" || raw_write="no"
        echo "raw_usb_access: read=${raw_read} write=${raw_write}"
      else
        echo "raw_usb: expected ${raw_node}, but it does not exist"
      fi
    fi

    interfaces=("${usb_device}":*)
    if ((${#interfaces[@]} == 0)); then
      echo "interfaces: none"
    fi
    for interface in "${interfaces[@]}"; do
      [[ -d "${interface}" ]] || continue
      echo "interface: ${interface}"
      attribute "${interface}/bInterfaceNumber" "interface_number"
      attribute "${interface}/bInterfaceClass" "interface_class"
      attribute "${interface}/bInterfaceSubClass" "interface_subclass"
      attribute "${interface}/bInterfaceProtocol" "interface_protocol"
      if [[ -L "${interface}/driver" ]]; then
        echo "driver: $(basename "$(readlink -f "${interface}/driver")")"
      else
        echo "driver: none"
      fi
      attribute "${interface}/modalias" "modalias"
    done

    for tty_class in /sys/class/tty/*; do
      [[ -e "${tty_class}/device" ]] || continue
      tty_device="$(readlink -f "${tty_class}/device" 2>/dev/null || true)"
      [[ "${tty_device}" == "${usb_device_real}" || "${tty_device}" == "${usb_device_real}/"* ]] || continue
      tty_node="/dev/$(basename "${tty_class}")"
      tty_nodes+=("${tty_node}")
    done
  done
fi

section "Serial devices"
if ((${#tty_nodes[@]} == 0)); then
  echo "No tty device is associated with ${ant_vid}:${ant_pid}."
  echo "BlakeBike's current ANT backend only enumerates serial ports, so it cannot open a raw-USB-only stick."
else
  for tty_node in "${tty_nodes[@]}"; do
    echo "tty: ${tty_node}"
    if [[ -e "${tty_node}" ]]; then
      stat -Lc 'permissions: %A %U:%G %n' "${tty_node}" 2>&1 || true
      [[ -r "${tty_node}" ]] && tty_read="yes" || tty_read="no"
      [[ -w "${tty_node}" ]] && tty_write="yes" || tty_write="no"
      echo "access: read=${tty_read} write=${tty_write}"
      show_udev_properties "/sys/class/tty/$(basename "${tty_node}")"
      echo "holders:"
      show_holders "${tty_node}"
    else
      echo "Device node is missing"
    fi
  done
fi

section "Raw USB process holders"
if ((${#raw_usb_nodes[@]} == 0)); then
  echo "No raw USB node was resolved"
else
  for raw_node in "${raw_usb_nodes[@]}"; do
    echo "node: ${raw_node}"
    [[ -e "${raw_node}" ]] && show_holders "${raw_node}" || true
  done
fi

section "Relevant kernel modules"
if command -v lsmod >/dev/null 2>&1; then
  lsmod | grep -E '^(usbserial|cp210x|cdc_acm|usbcore|usbhid)[[:space:]]' || echo "No common USB serial modules are loaded"
else
  echo "lsmod is unavailable"
fi

section "Installed BlakeBike udev rules"
rules=(/etc/udev/rules.d/*blakebike*ant*.rules /lib/udev/rules.d/*blakebike*ant*.rules /usr/lib/udev/rules.d/*blakebike*ant*.rules)
if ((${#rules[@]} == 0)); then
  echo "No BlakeBike ANT udev rule found"
else
  for rule in "${rules[@]}"; do
    echo "-- ${rule} --"
    sed -n '1,120p' "${rule}" 2>&1 || true
  done
fi

section "Recent USB kernel messages"
kernel_pattern='ANT|Dynastream|0fcf|1008|usbserial|cp210|ttyUSB|ttyACM'
if command -v journalctl >/dev/null 2>&1; then
  journalctl -k --since '-30 minutes' --no-pager 2>&1 \
    | grep -Ei "${kernel_pattern}" \
    | tail -100 || echo "No matching journal messages (or access was denied)"
elif command -v dmesg >/dev/null 2>&1; then
  dmesg --color=never 2>&1 \
    | grep -Ei "${kernel_pattern}" \
    | tail -100 || echo "No matching dmesg messages (or access was denied)"
else
  echo "Neither journalctl nor dmesg is available"
fi

section "Recent BlakeBike ANT log lines"
data_root="${XDG_DATA_HOME:-${HOME}/.local/share}"
log_dir="${data_root}/com.bpeterman.blakebike/logs"
app_logs=("${log_dir}"/blakebike*.log)
if ((${#app_logs[@]} == 0)); then
  echo "No BlakeBike logs found in ${log_dir/#${HOME}/~}"
else
  latest_log=""
  for candidate_log in "${app_logs[@]}"; do
    if [[ -z "${latest_log}" || "${candidate_log}" -nt "${latest_log}" ]]; then
      latest_log="${candidate_log}"
    fi
  done
  echo "log: ${latest_log/#${HOME}/~}"
  grep -Ei 'ANT|USBStick|ant_found|serial|permission denied|stick busy' "${latest_log}" \
    | tail -120 \
    | sed "s|${HOME}|~|g" || echo "No matching ANT log lines"
fi

section "Summary"
echo "usb_devices: ${#usb_devices[@]}"
echo "raw_usb_nodes: ${#raw_usb_nodes[@]}"
echo "associated_tty_nodes: ${#tty_nodes[@]}"
if ((${#usb_devices[@]} == 0)); then
  echo "result: dongle_not_visible_to_linux"
elif ((${#tty_nodes[@]} == 0)); then
  echo "result: raw_usb_visible_but_serial_backend_incompatible"
else
  inaccessible=0
  for tty_node in "${tty_nodes[@]}"; do
    [[ -r "${tty_node}" && -w "${tty_node}" ]] || inaccessible=$((inaccessible + 1))
  done
  if ((inaccessible > 0)); then
    echo "result: serial_device_permission_problem"
  else
    echo "result: serial_device_visible_and_accessible"
  fi
fi

echo
echo "Paste this complete output back into the shared troubleshooting note."
