#!/bin/sh
# Every crate in the tree is reviewed and permissively licensed.
#
# Fails when a dependency appears that DEPENDENCIES.md does not list, or when
# any crate carries a GPL-family licence: cairn is ISC and stays that way.
set -eu
cd "$(dirname "$0")/.."

report=$(mktemp)
trap 'rm -f "$report"' EXIT

cargo metadata --format-version 1 --locked \
	| jq -r '.packages[] | select(.source != null) | "\(.name)\t\(.license // "UNKNOWN")"' \
	| sort -u > "$report"

status=0
while IFS="$(printf '\t')" read -r name license; do
	if ! grep -q "^| \`$name\`" DEPENDENCIES.md; then
		echo "check-deps: $name is not reviewed in DEPENDENCIES.md" >&2
		status=1
	fi
	case "$license" in
	*GPL*)
		echo "check-deps: $name is $license; no GPL-family code enters this tree" >&2
		status=1
		;;
	*UNKNOWN*)
		echo "check-deps: $name declares no licence" >&2
		status=1
		;;
	esac
done < "$report"

count=$(wc -l < "$report" | tr -d ' ')
if [ "$status" -eq 0 ]; then
	echo "check-deps: $count third-party crates, all reviewed and permissive"
fi
exit "$status"
