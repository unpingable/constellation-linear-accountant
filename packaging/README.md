# linear-accountant-inference packaging

`la_inference` (docs/INFERENCE_ACCOUNTING.md) as an inert Jammy package:

| File | Installed as |
|---|---|
| `la_inference` (release, default `inference` feature) | `/usr/bin/la_inference` |
| `INFERENCE_ACCOUNTING.md`, this README | `/usr/share/doc/linear-accountant-inference/` |

The postinst creates `/var/lib/linear-accountant` (root:root 0700), the
default store directory. Nothing is enrolled: the owner runs
`la_inference enroll --file OWNER_FILE` once (root-owned, not group/other
writable). Purge never removes the books.

## Build (hermetic, Ubuntu 22.04)

Vendor a `git archive` export on the host, then build inside
`jammy-packager:1` with `--network none` (`rusqlite` is bundled and built
with the image's `cc`):

```sh
commit=$(git rev-parse HEAD); src=$(mktemp -d)
git archive "$commit" | tar -x -C "$src"
( cd "$src" && mkdir -p .cargo && \
  cargo +1.94.0 vendor --locked vendor > .cargo/config.toml )
docker run --rm --network none -v "$src":/work -w /work \
  -e RUSTUP_TOOLCHAIN=1.94.0 \
  -e SOURCE_DATE_EPOCH="$(git log -1 --format=%ct "$commit")" \
  jammy-packager:1 bash -c '
    cargo build --release --locked --offline --bin la_inference &&
    packaging/build-deb.sh 0.0.0+v2 amd64 target/release dist;
    rc=$?; chown -R '"$(id -u):$(id -g)"' /work; exit $rc'
```

Books do not move between hosts: a copied store is refused
(`store_identity`). Carrying a milestone's remaining stock to a rebuilt host
is an owner re-enrollment of at most the reconciled remainder, citing the
previous admission; never a second deposit of the original allocation.


## Executable build identity

The packaged native tools accept `--build-info`, `--version` and `-V` as sole
arguments before configuration, stdin or state access. These probes report
compile-time source/build declarations, compiler, target and profile. Missing
development declarations remain JSON null and cannot be packaged.

Export `LA_GIT_COMMIT` as the full lowercase 40-digit source commit and
`LA_BUILD_IDENTITY` as a unique build label (1..256 printable ASCII bytes
without spaces) for compilation and assembly. The builder must verify the source
export and locked toolchain inputs; the probe does not independently attest them.
Malformed nonempty declarations fail compilation. Assembly probes the staged
copies, checks their declared identity, compiler/profile/architecture and version,
and embeds their SHA-256 hashes at `/usr/share/doc/` under the package name in
`build-identity.json`. A matching `.build-identity.json` sidecar accompanies the
package. Compare installed probes and hashes to those receipts.

Run `python3 -B -m unittest discover -s packaging -p test_build_identity.py` for
package correspondence and refusal fixtures. No authority, state or core protocol
semantics change. Existing legacy version interfaces remain available.
