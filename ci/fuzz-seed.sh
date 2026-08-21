#!/bin/sh
# Fold a fuzzing corpus back into the committed seeds.
#
# A corpus discarded at the end of every run only ever re-finds what the seeds
# already reach. The nightly job carries its corpus forward between runs; this
# is how what it found gets committed, so a pull-request run starts from it too.
#
# Usage: ci/fuzz-seed.sh [target ...]     (default: both)
set -eu
cd "$(dirname "$0")/.."

targets=${*:-"archive request"}

if ! command -v cargo-fuzz >/dev/null 2>&1; then
	echo "fuzz-seed: cargo-fuzz is not installed" >&2
	exit 1
fi

for target in $targets; do
	corpus="fuzz/corpus/$target"
	seeds="fuzz/seeds/$target"
	if [ ! -d "$corpus" ]; then
		echo "fuzz-seed: $corpus does not exist; run make fuzz first" >&2
		exit 1
	fi

	before=$(find "$seeds" -type f | wc -l | tr -d ' ')

	# Minimise first: the corpus is kept for its coverage, not its size, and
	# an unminimised one grows without bound across nightly runs.
	(cd fuzz && cargo +nightly fuzz cmin "$target")

	mkdir -p "$seeds"
	added=0
	for f in "$corpus"/*; do
		[ -f "$f" ] || continue
		# libFuzzer names inputs by their SHA-1, so the name is the identity.
		name=$(basename "$f")
		if [ ! -e "$seeds/$name" ]; then
			cp "$f" "$seeds/$name"
			added=$((added + 1))
		fi
	done

	after=$(find "$seeds" -type f | wc -l | tr -d ' ')
	echo "fuzz-seed: $target: $before -> $after seeds ($added new)"
done

echo "fuzz-seed: commit fuzz/seeds/ to carry these into pull-request runs"
