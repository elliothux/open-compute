#!/bin/sh
# Run the P0.2 Gate in a controlled, privileged Linux network fixture.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" -eq 0 ]; then set -- p0-2; fi
"$root/test/gate.py" "$@" --list >/dev/null
http_port=38080
tcp_port=38081
tls_port=38082
public_ipv4=93.184.216.34
public_ipv6=2606:4700:4700::1111
fixture_pid=""
fixture_dir=""

if [ "$(uname -s)" != Linux ]; then
  echo "the controlled egress fixture requires Linux" >&2
  exit 1
fi
if [ "${OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO:-}" != 1 ]; then
  echo "set OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1 to authorize temporary loopback and /etc/hosts changes" >&2
  exit 1
fi
for command in sudo ip python3 openssl; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "missing required command: $command" >&2
    exit 1
  }
done
if ip address show dev lo | grep -Fq "$public_ipv4" || ip address show dev lo | grep -Fq "$public_ipv6"; then
  echo "controlled public fixture address is already configured; refusing to mutate it" >&2
  exit 1
fi
if grep -Fq '# open-compute-p0-2-egress' /etc/hosts; then
  echo "controlled egress fixture hosts entry already exists; refusing to mutate it" >&2
  exit 1
fi

cleanup() {
  if [ -n "$fixture_pid" ]; then
    kill "$fixture_pid" 2>/dev/null || true
    attempts=0
    while kill -0 "$fixture_pid" 2>/dev/null; do
      attempts=$((attempts + 1))
      if [ "$attempts" -ge 50 ]; then
        kill -KILL "$fixture_pid" 2>/dev/null || true
        break
      fi
      sleep 0.1
    done
    wait "$fixture_pid" 2>/dev/null || true
  fi
  sudo sed -i '/# open-compute-p0-2-egress$/d' /etc/hosts || true
  sudo ip address del "$public_ipv4/32" dev lo 2>/dev/null || true
  sudo ip -6 address del "$public_ipv6/128" dev lo 2>/dev/null || true
  if [ -n "$fixture_dir" ] && [ -d "$fixture_dir" ]; then
    rm -f "$fixture_dir/ca-key.pem" "$fixture_dir/ca.pem" "$fixture_dir/ca.srl" \
      "$fixture_dir/cert.csr" "$fixture_dir/cert.pem" "$fixture_dir/key.pem"
    rmdir "$fixture_dir" 2>/dev/null || true
  fi
}
trap cleanup EXIT HUP INT TERM

sudo ip address add "$public_ipv4/32" dev lo
sudo ip -6 address add "$public_ipv6/128" dev lo
printf '%s\n' \
  "$public_ipv4 p0-2-public.test # open-compute-p0-2-egress" \
  "127.0.0.1 p0-2-private.test # open-compute-p0-2-egress" |
  sudo tee -a /etc/hosts >/dev/null

mkdir -p "$root/.temp"
fixture_dir=$(mktemp -d "$root/.temp/p0-2-egress.XXXXXX")
openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 1 \
  -subj '/CN=open-compute P0.2 test CA' \
  -addext 'basicConstraints=critical,CA:TRUE' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -keyout "$fixture_dir/ca-key.pem" -out "$fixture_dir/ca.pem" >/dev/null 2>&1
openssl req -newkey rsa:2048 -sha256 -nodes \
  -subj '/CN=p0-2-public.test' \
  -addext 'subjectAltName=DNS:p0-2-public.test' \
  -addext 'basicConstraints=critical,CA:FALSE' \
  -addext 'keyUsage=critical,digitalSignature,keyEncipherment' \
  -addext 'extendedKeyUsage=serverAuth' \
  -keyout "$fixture_dir/key.pem" -out "$fixture_dir/cert.csr" >/dev/null 2>&1
openssl x509 -req -in "$fixture_dir/cert.csr" \
  -CA "$fixture_dir/ca.pem" -CAkey "$fixture_dir/ca-key.pem" -CAcreateserial \
  -days 1 -sha256 -copy_extensions copy -out "$fixture_dir/cert.pem" >/dev/null 2>&1
export OPEN_COMPUTE_EGRESS_FIXTURE_CERT="$fixture_dir/cert.pem"
export OPEN_COMPUTE_EGRESS_FIXTURE_KEY="$fixture_dir/key.pem"
python3 "$root/test/p0-2-egress-fixture.py" &
fixture_pid=$!
python3 - "$public_ipv4" "$public_ipv6" "$http_port" "$tcp_port" "$tls_port" <<'PY'
import socket, sys, time
targets = [(socket.AF_INET, sys.argv[1]), (socket.AF_INET6, sys.argv[2])]
deadline = time.monotonic() + 5
for port in map(int, sys.argv[3:]):
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

export OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_URL="http://$public_ipv4:$http_port/ipv4"
export OPEN_COMPUTE_EGRESS_PUBLIC_IPV6_URL="http://[$public_ipv6]:$http_port/ipv6"
export OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME_URL="http://p0-2-public.test:$http_port/dns"
export OPEN_COMPUTE_EGRESS_REDIRECT_PRIVATE_URL="http://$public_ipv4:$http_port/redirect-private"
export OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME_URL="http://p0-2-private.test:$http_port/private"
export OPEN_COMPUTE_EGRESS_PUBLIC_IPV4_HOST="$public_ipv4"
export OPEN_COMPUTE_EGRESS_PUBLIC_IPV6_HOST="$public_ipv6"
export OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME="p0-2-public.test"
export OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME="p0-2-private.test"
export OPEN_COMPUTE_EGRESS_PUBLIC_TCP_PORT="$tcp_port"
export OPEN_COMPUTE_EGRESS_PUBLIC_TLS_PORT="$tls_port"
export OPEN_COMPUTE_EGRESS_TLS_CA_PATH="$fixture_dir/ca.pem"
"$root/test/gate.py" "$@"
