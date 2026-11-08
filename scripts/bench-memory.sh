#!/bin/bash
# Peak resident memory (MiB) of one-shot compression and decompression, oxbrotli against the
# reference CLI, at quality $QUALITY (default 4) and the window each file's size calls for (as
# `Options::for_size` picks it, at most 22): [QUALITY=5] scripts/bench-memory.sh FILE...
# Needs `cargo build --release --examples` first, and macOS's /usr/bin/time -l.
set -eu
q=${QUALITY:-4}
ox=target/release/examples
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
rss() {
    /usr/bin/time -l "$@" 2>"$tmp/time" >/dev/null
    awk '/maximum resident set size/ { printf "%.1f", $1 / 1048576 }' "$tmp/time"
}
# The smallest window w >= 10 with 2^w - 16 >= the size, at most 22.
window() {
    local size=$1 w=10
    while [ "$w" -lt 22 ] && [ $(( (1 << w) - 16 )) -lt "$size" ]; do w=$((w + 1)); done
    echo "$w"
}
printf '%-16s %3s %10s %10s %10s %10s\n' input w ox-comp br-comp ox-dec br-dec
for f in "$@"; do
    w=$(window "$(wc -c <"$f")")
    "$ox/compress" "$q" "$w" "$f" "$tmp/ox.br"
    oc=$(rss "$ox/compress" "$q" "$w" "$f" "$tmp/ox.br")
    od=$(rss "$ox/decompress" "$tmp/ox.br")
    bc=$(rss brotli -q "$q" -w "$w" -f -c "$f")
    brotli -q "$q" -w "$w" -f -c "$f" >"$tmp/br.br"
    bd=$(rss brotli -d -f -c "$tmp/br.br")
    printf '%-16s %3s %10s %10s %10s %10s\n' "$(basename "$f")" "$w" "$oc" "$bc" "$od" "$bd"
done
