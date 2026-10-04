#!/usr/bin/env bash
# Assemble linear-accountant-inference_VERSION_ARCH.deb from a release
# `la_inference` built from one source export (see README.md).
# Usage: packaging/build-deb.sh VERSION ARCH BIN_DIR OUT_DIR
# No enrollment, admission file, store or credential is packaged.
set -euo pipefail; export LC_ALL=C TZ=UTC; umask 022
[[ $# -eq 4 ]] || { echo "usage: $0 VERSION ARCH BIN_DIR OUT_DIR" >&2; exit 2; }
version=$1 arch=$2 bin_dir=$(cd "$3" && pwd -P) out_dir=$4
case "$version" in *[!0-9A-Za-z.+:~-]*|'') exit 2;; esac
case "$arch" in amd64|arm64) ;; *) exit 2;; esac
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
bin=$bin_dir/la_inference
[[ -x "$bin" ]] || { echo "missing binary $bin" >&2; exit 2; }
"$bin" version >/dev/null || { echo "la_inference version failed" >&2; exit 2; }
mkdir -p "$out_dir"; stage=$(mktemp -d "$out_dir/.stage.XXXXXX"); trap 'rm -rf "$stage"' EXIT
name=linear-accountant-inference_${version}_${arch}
d=$stage/$name
doc=$d/usr/share/doc/linear-accountant-inference
install -d -m 0755 "$d/DEBIAN" "$d/usr/bin" "$doc"
install -m 0755 "$bin" "$d/usr/bin/la_inference"
install -m 0644 "$root/docs/INFERENCE_ACCOUNTING.md" "$root/packaging/README.md" "$doc/"
install -m 0644 "$root/LICENSE" "$doc/copyright"
sed -e "s/@VERSION@/$version/g" -e "s/@ARCH@/$arch/g" "$root/packaging/debian/control.in" > "$d/DEBIAN/control"
for s in postinst postrm; do install -m 0755 "$root/packaging/debian/$s" "$d/DEBIAN/$s"; done
find "$d" -exec touch -h -d "@${SOURCE_DATE_EPOCH:-0}" {} +
dpkg-deb --root-owner-group --build "$d" "$out_dir/$name.deb" >/dev/null
( cd "$out_dir" && sha256sum "$name.deb" > "$name.deb.sha256" )
echo "built $out_dir/$name.deb"
