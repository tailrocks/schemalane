#!/bin/sh
# Fail unless rust-toolchain.toml mirrors the mise.toml rust pin: channel, profile, components and targets.
# mise.toml is the source; rust-toolchain.toml is read by rustup, rust-analyzer, the mbx editor shim and non-mise tools.
# An absent profile is rustup's "default" profile (not "minimal") on either side. Needs only mise. Read-only.
set -eu

mise_file=${MIRROR_MISE_TOML:-mise.toml}
rtt_file=${MIRROR_RTT_TOML:-rust-toolchain.toml}

[ -f "$mise_file" ] || { echo "rust-toolchain-mirror: $mise_file not found" >&2; exit 1; }
[ -f "$rtt_file" ] || { echo "rust-toolchain-mirror: $rtt_file not found (Rust repos keep both files)" >&2; exit 1; }

get() { mise config get --file "$1" "$2" 2>/dev/null || true; }
# '["b", "a"]' -> 'a,b'; '"x"' -> 'x'
norm() { tr -d ' "[]\r' | tr ',' '\n' | sed '/^$/d' | LC_ALL=C sort | paste -sd, -; }

# `rust = "1.100.0"` (string: mise prints the bare scalar) or `rust = { version = ... }` (table:
# mise prints its `key = value` lines; a version never contains `=`).
raw=$(get "$mise_file" tools.rust)
case "$raw" in
  '') echo "rust-toolchain-mirror: $mise_file has no tools.rust pin" >&2; exit 1 ;;
  *=*)
    m_channel=$(get "$mise_file" tools.rust.version | norm)
    m_profile=$(get "$mise_file" tools.rust.profile | norm)
    m_components=$(get "$mise_file" tools.rust.components | norm)
    m_targets=$(get "$mise_file" tools.rust.targets | norm)
    ;;
  *) m_channel=$(printf '%s' "$raw" | norm); m_profile=; m_components=; m_targets= ;;
esac
r_channel=$(get "$rtt_file" toolchain.channel | norm)
r_profile=$(get "$rtt_file" toolchain.profile | norm)
r_components=$(get "$rtt_file" toolchain.components | norm)
r_targets=$(get "$rtt_file" toolchain.targets | norm)
: "${m_profile:=default}"
: "${r_profile:=default}"

status=0
cmp_field() { # name mise-value rtt-value
  if [ "$2" != "$3" ]; then
    echo "rust-toolchain-mirror: $1 differs: $mise_file='$2' $rtt_file='$3'" >&2
    status=1
  fi
}
cmp_field channel "$m_channel" "$r_channel"
cmp_field profile "$m_profile" "$r_profile"
cmp_field components "$m_components" "$r_components"
cmp_field targets "$m_targets" "$r_targets"
[ "$status" -eq 0 ] || echo "rust-toolchain-mirror: update both files together (mise.toml is the source)" >&2
exit "$status"
