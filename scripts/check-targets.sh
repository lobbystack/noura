#!/usr/bin/env bash
# Type-check and lint the desktop app for Linux and Windows from any
# computer, macOS included, with no cross toolchain installed.
#
# `cargo check` and `cargo clippy` never link, so the C and C++ that
# build scripts compile for the other target don't need to exist. This
# script points those build scripts at stand-ins: a pkg-config that
# finds every library, a compiler, archiver and nasm that write empty
# files, and an empty transcribe.cpp install. Rust code gets checked in
# full; C code gets checked by nobody, so the real CI builds stay
# authoritative.
#
# Usage: scripts/check-targets.sh [cargo-subcommand] [targets...]
#   scripts/check-targets.sh                 # clippy, Linux and Windows
#   scripts/check-targets.sh check x86_64-unknown-linux-gnu
set -euo pipefail

subcommand="${1:-clippy}"
shift || true
targets=("$@")
if [ "${#targets[@]}" -eq 0 ]; then
  targets=(x86_64-unknown-linux-gnu x86_64-pc-windows-msvc)
fi

stubs="$(mktemp -d)"
trap 'rm -rf "$stubs"' EXIT

cat > "$stubs/pkg-config" <<'EOF'
#!/bin/sh
for arg in "$@"; do
  case "$arg" in
    --modversion) echo 99.0; exit 0 ;;
    --variable*) echo /usr; exit 0 ;;
  esac
done
echo ""
EOF

cat > "$stubs/clang" <<'EOF'
#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  case "$arg" in
    -o?*) out="${arg#-o}" ;;
    -Fo*) out="${arg#-Fo}" ;;
    --version|-v|-dumpversion) echo "clang version 99.0.0"; exit 0 ;;
    -E) echo "clang"; echo "__clang__ __GNUC__"; exit 0 ;;
  esac
  prev="$arg"
done
[ -n "$out" ] && : > "$out"
exit 0
EOF

cat > "$stubs/ar" <<'EOF'
#!/bin/sh
for arg in "$@"; do
  case "$arg" in
    -out:*|-OUT:*|/out:*|/OUT:*) : > "${arg#*:}"; exit 0 ;;
    *.a|*.lib) : > "$arg"; exit 0 ;;
  esac
done
exit 0
EOF

cp "$stubs/clang" "$stubs/nasm"
chmod +x "$stubs/pkg-config" "$stubs/clang" "$stubs/ar" "$stubs/nasm"
mkdir -p "$stubs/transcribe/lib"
echo '{"shared":false,"libraries":[]}' > "$stubs/transcribe/lib/transcribe-link.json"

export PATH="$stubs:$PATH"
export PKG_CONFIG="$stubs/pkg-config" PKG_CONFIG_ALLOW_CROSS=1
export TRANSCRIBE_DIR="$stubs/transcribe"
# sccache doesn't know the stand-in compiler.
export RUSTC_WRAPPER=
export CARGO_INCREMENTAL=0

lint=()
if [ "$subcommand" = "clippy" ]; then
  lint=(-- -D warnings)
fi

for target in "${targets[@]}"; do
  key="${target//-/_}"
  export "CC_$key=$stubs/clang" "CXX_$key=$stubs/clang" "AR_$key=$stubs/ar"
  echo "== $target"
  cargo "$subcommand" -p desktop -p speech --target "$target" "${lint[@]}"
done
