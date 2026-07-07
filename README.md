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
cargo install --path .
```

This puts the `no-more-termius` binary in `~/.cargo/bin`. If that directory
is not already on your PATH, add it to your shell profile:

```sh
# ~/.zshrc or ~/.bashrc
export PATH="$HOME/.cargo/bin:$PATH"
```

Then run it directly from any terminal:

```sh
no-more-termius
```

### tmux shortcut (optional)

If you use tmux, a binding like this opens the picker in a new window —
pick a connection, ssh runs in that window, and when the session ends you
are back in the picker:

```tmux
# ~/.tmux.conf or ~/.config/tmux/tmux.conf
bind h new-window -n ssh ~/.cargo/bin/no-more-termius
```

Reload with `tmux source-file <path-to-your-conf>` and use `prefix + h`.

## Data

Connections live in `~/.config/no-more-termius/connections.toml`. On first
run the app offers to import the hosts from `~/.ssh/config` (wildcard entries
are skipped). Connecting uses **system ssh**, so your `~/.ssh/config`
directives (agent, ProxyJump, IdentityAgent…) still apply on top.

## Keys

Press `?` on any screen for contextual help — the bindings for the current
screen are listed first.

### Connection list
Vim-style: single letters are actions; `/` focuses the fuzzy search.
Favorites (`★`) sort first, then most recently used.

| Key | Action |
|---|---|
| `/` | fuzzy-search (typing filters, Enter connects, Esc leaves) |
| Enter | connect (ssh) |
| `s` | open SFTP browser |
| `j`/`k`, ↑/↓, `g`/`G` | move selection |
| `a` / `e` / `d` | add / edit / delete (confirms) |
| `y` | duplicate connection (opens pre-filled form) |
| `f` | toggle favorite |
| Esc | clear search, then quit · `q` / Ctrl-c quit |

### Form
Tab/Shift-Tab move between fields. On the *Key file* field, **Ctrl-o** opens a
file browser starting at `~` (hidden files visible — keys live in `~/.ssh`;
`.` toggles). `/` filters, `h`/`l` or Enter navigate, Enter on a file picks
it. The optional *SFTP folder* field sets the remote directory the SFTP
browser opens in (`~/...` is home-relative; falls back to home if missing).
Enter saves the form, Esc cancels.

### SFTP (dual pane: local left, remote right)
`/` filters the active pane. Tab switches panes.

| Key | Action |
|---|---|
| Enter / `l` on dir | enter it |
| Enter on file | transfer it to the other pane's directory |
| Space | preview in a modal — text scrollable, images rendered in-terminal (pure Rust, no external tools). Remote files: images/text up to 10 MB, temp copies removed automatically |
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
ssh-agent → default `~/.ssh/id_*` keys.

## Development

```sh
cargo test      # parser, store, ssh args, pane logic
cargo clippy
```

Branching: `main` holds tagged releases (`vX.Y.Z`); day-to-day work happens
on `develop` and is merged into `main` for each release.
