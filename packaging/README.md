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
