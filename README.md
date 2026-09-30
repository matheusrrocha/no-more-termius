# no-more-termius

Keyboard-driven TUI to manage and open SSH connections, with a dual-pane
SFTP browser. Built with Rust + ratatui.

## Why

Termius kept removing my saved hosts after updates. After the third time
re-adding every server by hand, I got tired of it and built this: my
connections now live in a plain TOML file that I own, version, and back up —
no account, no sync, no surprises.

## Install

```sh
cargo install no-more-termius
```

Pin a version with `cargo install no-more-termius@<version>`. Building needs a
Rust toolchain (1.85+) and, on Linux, the OpenSSL and zlib headers your
distribution ships (`openssl-devel`/`libssl-dev`, `zlib-devel`/`zlib1g-dev`).

### Arch Linux

```sh
yay -S no-more-termius
```

Also covers Arch-based distributions such as Omarchy. The `PKGBUILD` lives in
[`packaging/aur`](packaging/aur).

### From source

Straight from the repository, without cloning. `main` only ever holds tagged
releases, so this tracks the latest one:

```sh
cargo install --git https://github.com/matheusrrocha/no-more-termius
```

From a clone, to run a branch or your own changes:

```sh
git clone https://github.com/matheusrrocha/no-more-termius
cd no-more-termius
cargo install --path .
```

Any of these put the `no-more-termius` binary in `~/.cargo/bin`. If that
directory is not already on your PATH, add it to your shell profile:

```sh
# ~/.zshrc or ~/.bashrc
export PATH="$HOME/.cargo/bin:$PATH"
```

Then run it from any terminal:

```sh
no-more-termius
```

### tmux shortcut (optional)

If you use tmux, a binding like this opens the picker in a new window —
pick a connection and ssh takes over that window, so the window lives
exactly as long as the session does:

```tmux
# ~/.tmux.conf or ~/.config/tmux/tmux.conf
bind h new-window -n ssh ~/.cargo/bin/no-more-termius
```

Reload with `tmux source-file <path-to-your-conf>` and use `prefix + h`.

For pixel-perfect image previews inside tmux, also enable passthrough:

```tmux
set -g allow-passthrough on
```

### Clipboard (optional)

`y` copies through whatever helper your OS ships: `pbcopy` on macOS, `clip`
on Windows, and `wl-copy` (Wayland) / `xclip` / `xsel` (X11) on Linux and
BSD — install one from your package manager if you don't have it, e.g.
`pacman -S wl-clipboard` or `apt install xclip`. With none of them present
the copy falls back to the terminal's OSC 52 sequence, which also puts the
text on your *local* clipboard when the app itself is running over ssh; the
status line says when that path was used, since terminals may ignore it.

## Data

Connections live in `~/.config/no-more-termius/connections.toml`. On first
run the app offers to import the hosts from `~/.ssh/config` (wildcard entries
are skipped). Connecting uses **system ssh**, so your `~/.ssh/config`
directives (agent, ProxyJump, IdentityAgent…) still apply on top. The app
replaces itself with ssh when you connect: the session owns the terminal
outright and ending it returns you to your shell, not to the picker.

Every session runs with `ServerAliveInterval=15` / `ServerAliveCountMax=3`,
so a connection that died while the machine slept ends within ~45 seconds
instead of freezing the terminal. The SFTP browser likewise reports
"connection lost" and returns to the list.

A connection's optional **password** is stored in `connections.toml` in the
clear (the file is kept at mode `0600`) and travels inside encrypted export
bundles. On connect, ssh is pointed at the app itself as `SSH_ASKPASS`: the
first password prompt is answered for you, and anything else — host-key
questions, key passphrases, 2FA codes, or a retry after a wrong stored
password — is asked on the terminal as usual. Needs OpenSSH 8.4+.

## Keys

Press `?` on any screen for contextual help — the bindings for the current
screen are listed first.

### Connection list
Vim-style: single letters are actions; `/` focuses the fuzzy search.
Favorites (`★`) sort first, then most recently used.

| Key | Action |
|---|---|
| `/` | fuzzy-search (typing filters, Enter connects, Esc leaves) |
| Enter | connect (ssh) — the app exits into the session |
| `s` | open SFTP browser |
| `j`/`k`, ↑/↓, `g`/`G` | move selection |
| `a` / `e` / `d` | add / edit / delete (confirms) |
| `y` | duplicate connection (opens pre-filled form) |
| `f` | toggle favorite |
| `x` / `i` | export / import an encrypted bundle |
| `r` | reload the list from disk (also automatic) |
| Esc | clear search, then quit · `q` / Ctrl-c quit |

### Form
Tab/Shift-Tab move between fields. On the *Key file* field, **Ctrl-o** opens a
file browser starting at `~` (hidden files visible — keys live in `~/.ssh`;
`.` toggles). `/` filters, `h`/`l` or Enter navigate, Enter on a file picks
it. The *Password* field is masked (**Ctrl-r** shows it; `?` types a literal
`?` there). The optional *SFTP folder* field sets the remote directory the SFTP
browser opens in (`~/...` is home-relative; falls back to home if missing).
Enter saves the form, Esc cancels.

### Moving to another machine

`x` exports every connection **and the private key files they point at** into
a single encrypted bundle; `i` on the other machine puts them back. The
passphrase you type on export is the only thing that opens the file — there is
no recovery, no account, nothing phones home.

| Field | Meaning |
|---|---|
| Bundle file | where to write it / which one to read (`~` works, Ctrl-o browses) |
| Passphrase | at least 8 characters, typed twice on export |
| Restore keys into | import only — defaults to `~/.ssh`, Ctrl-o then `p` picks another folder |

Keys are restored under their original file name with `0600` permissions
(`.pub` siblings come along at `0644`), and each connection is repointed at
its copy on the new machine — the two machines do not have to keep keys in the
same place. Names are kept unique: `~/.ssh/id_rsa` and `~/chaves/id_rsa` are
different keys, so the second arrives as `id_rsa-2`.

Nothing is destroyed without a yes:

- A connection already present with the same name **and** the same
  host/port/user is skipped, so re-importing the same bundle changes nothing.
- A name that clashes with a *different* server arrives as `name (imported)`.
- A key file already at the target path with different contents is left alone
  unless you answer `o` (overwrite) instead of `y`.
- Exporting onto an existing file asks first.

The file is armored text, safe to email to yourself or drop in cloud storage:
Argon2id (64 MiB, 3 passes) derives the key from your passphrase, and
XChaCha20-Poly1305 seals the contents, so a wrong passphrase or a single
modified byte is refused rather than silently mis-decrypted.

```
-----BEGIN NO-MORE-TERMIUS BUNDLE-----
Tk1UQk5ETAEAAAEAAwAAAAEAAADeXj0K+55v5lpXTL+NygPOZsAdrk8gLzjiU96d
...
-----END NO-MORE-TERMIUS BUNDLE-----
```

### SFTP (dual pane: local left, remote right)
`/` filters the active pane. Tab switches panes.

| Key | Action |
|---|---|
| Enter / `l` on dir | enter it |
| Enter on file | transfer it to the other pane's directory |
| Space | preview in a modal — text scrollable (`y` copies it); images pixel-perfect via the Kitty (kitty, Ghostty, WezTerm) or iTerm2 graphics protocol, with a pure-Rust quadrant-block fallback elsewhere. Remote files: images/text up to 10 MB, temp copies removed automatically |
| `y` | copy selected path to clipboard |
| `R` | rename selection |
| `D` | delete selection (confirms; dirs must be empty) |
| `h` / Backspace | parent directory |
| `j`/`k`, ↑/↓, PgUp/PgDn, `g`/`G` | move selection |
| `.` | show/hide hidden files |
| `r` | refresh both panes |
| Esc | cancel transfer / clear filter / leave · `q` leave |

**Overwrites always ask for confirmation** (default is *no*). Transfers write
to a `.part` file and rename on completion, so a cancelled or failed transfer
never corrupts an existing destination file. Unknown host keys prompt with the
SHA256 fingerprint before being appended to `~/.ssh/known_hosts`; mismatching
keys abort. Auth order: connection key file (passphrase prompted if needed) →
stored password (password or keyboard-interactive) → ssh-agent → default `~/.ssh/id_*` keys.

## Development

```sh
cargo test      # parser, store, ssh args, pane logic, bundle crypto
cargo clippy
```

Branching: `main` holds tagged releases (`vX.Y.Z`); day-to-day work happens
on `develop` and is merged into `main` for each release.
