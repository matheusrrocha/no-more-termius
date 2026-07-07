# termius-tui

Keyboard-driven TUI to manage and open SSH connections, with a dual-pane
SFTP browser. Built with Rust + ratatui.

## Install

```sh
cargo install --path .        # → ~/.cargo/bin/termius-tui
```

tmux shortcut (already configured in `~/.config/tmux/tmux.conf`):

```tmux
bind h new-window -n ssh ~/.cargo/bin/termius-tui
```

`prefix + h` opens a new window with the connection picker. Picking a
connection runs `ssh` in that window; when the session ends you are back in
the picker.

## Data

Connections live in `~/.config/termius-tui/connections.toml`. On first run
the app offers to import the hosts from `~/.ssh/config` (wildcard entries are
skipped). Connecting uses **system ssh**, so your `~/.ssh/config` directives
(agent, ProxyJump, IdentityAgent…) still apply on top.

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
it. Enter saves the form, Esc cancels.

### SFTP (dual pane: local left, remote right)
`/` filters the active pane. Tab switches panes.

| Key | Action |
|---|---|
| Enter / `l` on dir | enter it |
| Enter on file | transfer it to the other pane's directory |
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
