#!/usr/bin/env bash
set -euo pipefail

scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

cargo package --package a2ui-rs --all-features --locked --offline --no-verify --target-dir "$scratch"
shopt -s nullglob
archives=("$scratch"/package/a2ui-rs-*.crate)
if [[ ${#archives[@]} -ne 1 ]]; then
  printf '%s\n' 'Expected one a2ui-rs .crate archive.' >&2
  exit 1
fi

archive=${archives[0]}
package_name=$(basename "$archive" .crate)
tar -xzf "$archive" -C "$scratch"
package="$scratch/$package_name"
for file in README.md LICENSE; do
  if [[ ! -s "$package/$file" ]] || ! cmp -s "$file" "$package/$file"; then
    printf 'The crate archive must contain the current %s.\n' "$file" >&2
    exit 1
  fi
done

consumer="$scratch/consumer"
mkdir -p "$consumer/src"
cat > "$consumer/Cargo.toml" <<EOF_MANIFEST
[package]
name = "a2ui-package-consumer"
version = "0.0.0"
edition = "2024"

[dependencies]
a2ui-rs = { path = "../$package_name" }
EOF_MANIFEST
cp "$package/Cargo.lock" "$consumer/Cargo.lock"

awk '
  /^## Quick start$/ { quick_start = 1; next }
  /^## / && quick_start { exit }
  quick_start && /^```rust$/ { rust = 1; next }
  rust && /^```$/ { exit }
  rust { print }
' "$package/README.md" > "$consumer/src/main.rs"
if [[ ! -s "$consumer/src/main.rs" ]]; then
  printf '%s\n' 'The packaged README must contain a Rust quick start.' >&2
  exit 1
fi

cd "$consumer"
cargo build --offline

cat > src/main.rs <<'EOF_RUST'
use a2ui_rs::{basic_catalog::register_validation_functions, resolution::LocalFunctions};

fn main() {
    let mut functions = LocalFunctions::default();
    register_validation_functions(&mut functions, "example:catalog");
}
EOF_RUST
cargo build --offline --features a2ui-rs/basic-catalog
