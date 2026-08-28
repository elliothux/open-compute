#!/bin/sh
# Run the P0.2 Gate in a controlled, privileged Linux network fixture.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
port=38080
public_ipv4=93.184.216.34
public_ipv6=2606:4700:4700::1111
fixture_pid=""

if [ "$(uname -s)" != Linux ]; then
  echo "the controlled egress fixture requires Linux" >&2
  exit 1
fi
if [ "${OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO:-}" != 1 ]; then
  echo "set OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1 to authorize temporary loopback and /etc/hosts changes" >&2
  exit 1
fi
for command in sudo ip python3; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "missing required command: $command" >&2
    exit 1
  }
done
if ip address show dev lo | grep -Fq "$public_ipv4" || ip address show dev lo | grep -Fq "$public_ipv6"; then
  echo "controlled public fixture address is already configured; refusing to mutate it" >&2
  exit 1
fi

cleanup() {
  if [ -n "$fixture_pid" ]; then
    kill "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  sudo sed -i '/# open-compute-p0-2-egress$/d' /etc/hosts || true
  sudo ip address del "$public_ipv4/32" dev lo 2>/dev/null || true
  sudo ip -6 address del "$public_ipv6/128" dev lo 2>/dev/null || true
}
trap cleanup EXIT HUP INT TERM

sudo ip address add "$public_ipv4/32" dev lo
sudo ip -6 address add "$public_ipv6/128" dev lo
printf '%s\n' \
  "$public_ipv4 p0-2-public.test # open-compute-p0-2-egress" \
  "127.0.0.1 p0-2-private.test # open-compute-p0-2-egress" |
  sudo tee -a /etc/hosts >/dev/null

python3 "$root/test/p0-2-egress-fixture.py" &
fixture_pid=$!
python3 - "$public_ipv4" "$public_ipv6" "$port" <<'PY'
import socket, sys, time
targets = [(socket.AF_INET, sys.argv[1]), (socket.AF_INET6, sys.argv[2])]
port = int(sys.argv[3])
deadline = time.monotonic() + 5
for family, address in targets:
    while True:
        try:
            with socket.socket(family, socket.SOCK_STREAM) as sock:
                sock.settimeout(0.25)
                sock.connect((address, port))
            break
        except OSError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.05)
PY

export OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_URL="http://$public_ipv4:$port/ipv4"
export OPEN_COMPUTE_EGRESS_PUBLIC_IPV6_URL="http://[$public_ipv6]:$port/ipv6"
export OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME_URL="http://p0-2-public.test:$port/dns"
export OPEN_COMPUTE_EGRESS_REDIRECT_PRIVATE_URL="http://$public_ipv4:$port/redirect-private"
export OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME_URL="http://p0-2-private.test:$port/private"
"$root/test/test-p0-2.sh"
