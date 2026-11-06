#!/usr/bin/env bash
# Policy checks the compiler and cargo-deny don't cover. Run: scripts/check-policy.sh
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
problem() { echo "policy: $*" >&2; fail=1; }

grep -q '^unsafe_code = "forbid"$' Cargo.toml || problem 'Cargo.toml must set unsafe_code = "forbid"'
grep -q '^#!\[forbid(unsafe_code)\]$' src/lib.rs || problem 'src/lib.rs must #![forbid(unsafe_code)]'

# No unsafe anywhere in our sources, and no attempt to re-allow it.
if grep -rnE '\bunsafe[[:space:]]*(\{|fn\b|impl\b|trait\b|extern\b)|allow\(unsafe_code\)|expect\(unsafe_code\)' \
    src tests benches fuzz --include='*.rs' --exclude-dir=target 2>/dev/null; then
    problem "unsafe code found"
fi

# No dependencies at all: [dependencies] and [dev-dependencies] stay empty.
for section in dependencies dev-dependencies build-dependencies; do
    if awk -v s="[$section]" '$0==s{f=1; next} /^\[/{f=0} f && NF && $0 !~ /^#/' Cargo.toml | grep -q .; then
        problem "Cargo.toml [$section] must be empty (no dependencies, no C)"
    fi
done

# No C: no build script, and no cc/bindgen anywhere.
[ ! -e build.rs ] || problem "build.rs found: nothing is built besides Rust"
if grep -rnE '\b(cc|bindgen|libc)\b *=' Cargo.toml; then
    problem "C tooling in Cargo.toml"
fi

# Nothing compiled in from outside the crate: include!, include_bytes! and include_str! take
# paths next to the file or below it, never absolute ones or ones with "..".
if grep -rnE 'include(_bytes|_str)?!\([[:space:]]*"(/|[^"]*\.\.)' \
    src tests benches examples --include='*.rs' 2>/dev/null; then
    problem "include! of a path outside the crate"
fi

if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo "policy: ok"
