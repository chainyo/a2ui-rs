#!/usr/bin/env bash
set -euo pipefail

version=0.1.9
case "$(uname -s):$(uname -m)" in
  Linux:x86_64)
    target=x86_64-unknown-linux-gnu
    checksum=027124444baddf7fa3597ce2d76d1eff48e95902be303beada2558e3687c1bff
    ;;
  Linux:aarch64|Linux:arm64)
    target=aarch64-unknown-linux-gnu
    checksum=bf2b31d180e7716eb69134c4340d6a708b82f16570c7e06c2dd68664db4ab438
    ;;
  Darwin:arm64)
    target=aarch64-apple-darwin
    checksum=0699ef6d4134a1b6902445c5162ce273b76b3ffe7cb451da7b5d359084f87237
    ;;
  Darwin:x86_64)
    target=x86_64-apple-darwin
    checksum=d42252dafc94aa741a5d559f67b3016d8d7faa57f5cc765fb71fac3f7c1f1ae9
    ;;
  *) echo "Unsupported Hawk platform" >&2; exit 1 ;;
esac

archive="cargo-hawk-${target}"
temporary_directory="$(mktemp -d)"
trap 'rm -rf "$temporary_directory"' EXIT
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  --retry 2 --connect-timeout 15 --max-time 120 \
  "https://github.com/astral-sh/hawk/releases/download/${version}/${archive}.tar.gz" \
  --output "${temporary_directory}/archive.tar.gz"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${temporary_directory}/archive.tar.gz")"
else
  actual="$(shasum -a 256 "${temporary_directory}/archive.tar.gz")"
fi
if [[ "${actual%% *}" != "$checksum" ]]; then
  echo "Hawk archive checksum mismatch" >&2
  exit 1
fi
tar -xzf "${temporary_directory}/archive.tar.gz" -C "$temporary_directory" \
  "${archive}/cargo-hawk" "${archive}/cargo-hawk-driver"
destination="${CARGO_HOME:-${HOME}/.cargo}/bin"
mkdir -p "$destination"
install -m 0755 "${temporary_directory}/${archive}/cargo-hawk" "$destination/cargo-hawk"
install -m 0755 "${temporary_directory}/${archive}/cargo-hawk-driver" "$destination/cargo-hawk-driver"
echo "Installed Hawk ${version}. Add ${destination} to PATH."
