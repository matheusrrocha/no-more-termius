# AUR package (prebuilt)

`no-more-termius-bin` installs the binary attached to the GitHub release
instead of compiling from source — seconds instead of minutes, at the cost of
trusting our CI build rather than your own machine. It `provides` and
`conflicts` with `no-more-termius`, so the two are interchangeable and cannot
both be installed.

The source package lives in [`../aur`](../aur); everything there about AUR
setup and pushing applies here, with `no-more-termius-bin` as the repo name.

## Where the binaries come from

[`.github/workflows/release.yml`](../../.github/workflows/release.yml) builds
`x86_64` and `aarch64` on a tag push and attaches:

```
no-more-termius-<version>-<target>-unknown-linux-gnu.tar.gz
no-more-termius-<version>-<target>-unknown-linux-gnu.tar.gz.sha256
```

They are built on Ubuntu 22.04 because glibc is only forward-compatible: a
binary built against its older glibc runs on Arch, but not the other way
round. OpenSSL, libssh2 and zlib are linked, not vendored — an ssh tool should
pick up the distribution's security updates for its transport rather than
freeze them in.

## Publishing

`sha256sums_*` are committed as `SKIP` because the assets do not exist until
the release workflow has run. Fill them in before pushing:

```sh
cd packaging/aur-bin
updpkgsums                       # downloads the assets, writes real sums
makepkg --printsrcinfo > .SRCINFO
makepkg -f                       # installs into a fakeroot, sanity-checks it
namcap PKGBUILD *.pkg.tar.zst
```

**Do not push with `SKIP`** — it disables checksum verification, so users would
install whatever the URL happens to serve.

The `depends` list mirrors the source package and was derived from the
binary's linkage. The workflow's *Report dynamic dependencies* step prints
`ldd` output for exactly this: check it against the list when the sonames
change.
