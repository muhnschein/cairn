#!/bin/sh
# Build the container image and prove it serves, rootless and read-only.
#
# Needs podman; without one this is a skip, like man-lint without mandoc.
# The assertions that matter: the daemon comes up under `sandbox = require`
# inside podman's own seccomp filter (the two compose), reports both
# confinement layers as applied, serves a crafted archive over the unix
# socket on the bind-mounted runtime directory, and answers suggestion
# queries. The quadlet file is parsed by the generator in dry-run mode so
# a typo cannot reach a production boot.
set -eu
cd "$(dirname "$0")/.."

if ! command -v podman >/dev/null 2>&1; then
	echo "container-smoke: podman not installed, skipping"
	exit 0
fi

TAG=$(grep -oE '"[0-9]{4}\.[0-9]{2}"' crates/api/src/lib.rs | head -1 | tr -d '"')
IMAGE=localhost/cairn:$TAG

echo "container-smoke: building $IMAGE"
podman build --quiet -t "$IMAGE" .

echo "container-smoke: crafting an archive"
rm -rf target/container-smoke
mkdir -p target/container-smoke/zim target/container-smoke/conf target/container-smoke/sock
cargo run --quiet -p testutil -- sample target/container-smoke/zim/sample.zim

cat > target/container-smoke/conf/cairn.conf <<'EOF'
listen = unix:/sock/cairn.sock
archive_dir = /srv/zim
sandbox = require
EOF

cleanup() {
	podman rm -f cairn-smoke >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

echo "container-smoke: running rootless, read-only, no network"
podman run -d --name cairn-smoke \
	--read-only --network none \
	--user 65532:65532 \
	-v "$PWD/target/container-smoke/zim:/srv/zim:ro" \
	-v "$PWD/target/container-smoke/conf/cairn.conf:/etc/cairn/cairn.conf:ro" \
	-v "$PWD/target/container-smoke/sock:/sock" \
	"$IMAGE" -c /etc/cairn/cairn.conf

sock=target/container-smoke/sock/cairn.sock
i=0
until [ -S "$sock" ] && curl -s --unix-socket "$sock" http://cairn/v1/status >/dev/null 2>&1; do
	i=$((i + 1))
	[ "$i" -gt 100 ] && {
		echo "container-smoke: daemon did not come up; logs:"
		podman logs cairn-smoke || true
		exit 1
	}
	sleep 0.3
done

status=$(curl -s --unix-socket "$sock" http://cairn/v1/status)
echo "$status"

echo "$status" | grep -q '"landlock"' || { echo "container-smoke: landlock layer missing"; exit 1; }
echo "$status" | grep -q '"seccomp"' || { echo "container-smoke: seccomp layer missing"; exit 1; }
# Applied, not merely attempted: a daemon that failed to confine itself
# otherwise looks identical to one that succeeded.
applied=$(echo "$status" | grep -oE '"state": *"applied"' | wc -l)
[ "$applied" -ge 2 ] || {
	echo "container-smoke: expected landlock and seccomp applied, saw $applied"
	exit 1
}
echo "$status" | grep -q '"archives": *1' || { echo "container-smoke: archive not opened"; exit 1; }

suggest=$(curl -s --unix-socket "$sock" 'http://cairn/v1/archives/63616972-6e2d-7465-7374-2d7575696431/suggest?q=Ma')
echo "$suggest"
echo "$suggest" | grep -q '"index\.html"' || { echo "container-smoke: suggestion query failed"; exit 1; }

echo "container-smoke: serving under require, both layers applied"

QUADLET=""
[ -x /usr/libexec/podman/quadlet ] && QUADLET=/usr/libexec/podman/quadlet
command -v quadlet >/dev/null 2>&1 && QUADLET=$(command -v quadlet)
if [ -n "$QUADLET" ]; then
	echo "container-smoke: parsing the quadlet with the generator"
	"$QUADLET" -dryrun systemd/cairnd.container >/dev/null
	grep -q 'Network=none' systemd/cairnd.container
	grep -q 'ReadOnly=yes' systemd/cairnd.container
	grep -q 'DropCapability=all' systemd/cairnd.container
else
	echo "container-smoke: quadlet generator not installed, skipping its parse"
fi

echo "container-smoke: ok"
