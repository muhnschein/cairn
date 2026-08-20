#!/bin/sh
# The crate boundaries from the scope, enforced rather than remembered.
#
#   zimfmt   parses hostile bytes and must not reach anything that does I/O
#   api      parses hostile requests and must have no dependencies at all
#   sandbox  must not depend on the rest of the workspace
#   archive  must not know about HTTP
#   cairn    the CLI is a plain client
set -eu
cd "$(dirname "$0")/.."

status=0

# Transitive normal dependencies of a crate, one name per line, itself excluded.
tree() {
	cargo tree --package "$1" --edges normal --prefix none --no-dedupe \
		| awk '{print $1}' | sort -u | grep -v "^$1\$" | grep -v '^$' || true
}

check() {
	crate=$1
	shift
	allowed=" $* "
	for dep in $(tree "$crate"); do
		case "$allowed" in
		*" $dep "*) ;;
		*)
			echo "check-boundaries: $crate must not depend on $dep" >&2
			status=1
			;;
		esac
	done
}

# lzma-rs and ruzstd decode bytes in memory; neither opens anything.
check zimfmt lzma-rs ruzstd byteorder crc crc-catalog
check api
check sandbox libc
check archive zimfmt memmap2 libc lzma-rs ruzstd byteorder crc crc-catalog
check cairn

if [ "$status" -eq 0 ]; then
	echo "check-boundaries: crate boundaries hold"
fi
exit "$status"
