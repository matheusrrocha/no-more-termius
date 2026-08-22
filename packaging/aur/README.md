# AUR package

Source of truth for the [`no-more-termius`](https://aur.archlinux.org/packages/no-more-termius)
AUR package. The AUR wants its own git repo containing only `PKGBUILD` and
`.SRCINFO` at the root, so these files are kept here and copied over on
release rather than the AUR repo being a submodule.

## First-time setup

Add an SSH key to your account at <https://aur.archlinux.org/account>, then:

```sh
git clone ssh://aur@aur.archlinux.org/no-more-termius.git aur-no-more-termius
```

An empty clone is normal — it means the name is still free and the first push
creates the package.

## Publishing a release

From an Arch machine (or an `archlinux` container), after the GitHub tag exists:

```sh
cd packaging/aur
updpkgsums                       # refresh sha256sums for the new tag
makepkg --printsrcinfo > .SRCINFO
makepkg -f                       # must build and pass tests
namcap PKGBUILD *.pkg.tar.zst    # must be quiet about deps

cp PKGBUILD .SRCINFO ../../../aur-no-more-termius/
cd ../../../aur-no-more-termius
git commit -am "no-more-termius 0.4.0"
git push
```

Users then install with `yay -S no-more-termius` (or `paru`, `pacman -U` after
a manual `makepkg -si`).

## Notes

- `pkgrel` goes back to `1` on every new `pkgver`, and increments when only the
  packaging changes.
- `depends` was derived from what the binary links against (openssl, zlib,
  libssh2 via the `ssh2` crate) and has **not** been verified with `namcap` on
  Arch — do that before the first push and correct the list if it complains.
- `libssh2` is listed on purpose: with it installed at build time `libssh2-sys`
  links the system copy instead of vendoring its own, which is what Arch wants.
