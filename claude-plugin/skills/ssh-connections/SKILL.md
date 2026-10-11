---
name: ssh-connections
description: Find how to reach a server over SSH and run commands on it, using the user's saved connections in no-more-termius. Use whenever a task needs SSH access to a server, host, VPS or box (by name, hostname or IP), asks which servers exist, or asks to add, edit, rename or remove a saved SSH connection.
---

# SSH connections (no-more-termius)

The user keeps their SSH servers in **no-more-termius**. Its binary answers
non-interactive commands; always go through it, never read or edit
`~/.config/no-more-termius/connections.toml` directly — that file holds
passwords in the clear, and the commands below keep them out of the
conversation.

If `no-more-termius` is not on PATH, try `~/.cargo/bin/no-more-termius`. If
neither exists, tell the user to install it (`cargo install no-more-termius`)
and stop. `no-more-termius help` lists every command.

## Find a server

```sh
no-more-termius list          # every connection, TOML, passwords redacted
no-more-termius show <name>   # one connection + the ssh command line for it
```

Names are exact and case-sensitive. When the user says "the prod box" or an
IP, run `list` and match on `name`/`host`; if several fit, ask which one.

Each entry shows `host`, `port`, `user`, `identity_file`, `sftp_dir`,
`favorite` and `has_password` (never the password itself).

## Run commands on a server

```sh
no-more-termius run <name> -- <remote command>
```

- Works for key **and** stored-password connections: the stored password is
  answered for you without ever being printed. Prefer this over a raw `ssh`.
- Exit code is the remote command's. Quote the remote command as one
  argument when it has pipes, `&&`, globs or redirections:
  `no-more-termius run web -- 'cd /srv/app && git log -1 --oneline'`.
- Each `run` is a fresh connection; no state (cwd, env) carries over.
- There is no TTY: interactive programs (`top`, `vim`, `sudo` asking for a
  password, a REPL) will hang or fail. Use non-interactive forms
  (`top -bn1`, `sudo -n`, `systemctl --no-pager`, `journalctl --no-pager -n 200`).
- If ssh fails on an **unknown host key**, a passphrase-protected key, or a
  2FA prompt, it cannot be answered from here. Ask the user to connect once
  from their own terminal (`no-more-termius`, pick the server) and then retry.
  Never disable host-key checking to get around it.
- For copying files with scp/rsync, use the `ssh` line from `show` for the
  host, user, port and key. That works only when `has_password = false`.

Treat remote commands with the same care as local ones: read-only
inspection is fine, but anything that changes or deletes things on a
server (restarts, deploys, `rm`, package installs, editing config) needs the
user's go-ahead first, unless they already asked for exactly that.

## Add, edit, remove

```sh
no-more-termius add --name <n> --host <h> [--port <p>] [--user <u>] \
    [--key <file>] [--sftp-dir <dir>] [--favorite] [--password-stdin]
no-more-termius edit <name> [any add option] \
    [--no-user] [--no-key] [--no-sftp-dir] [--no-password] [--no-favorite]
no-more-termius rm <name>
```

- `edit` changes only the fields given; `--name` renames.
- `--key` must point at an existing private key file (`~/` is expanded).
- Passwords are only accepted on stdin, so they stay out of the command
  line. Only set one when the user gives it to you for that purpose:
  `printf '%s\n' "$PW" | no-more-termius edit web --password-stdin`
  Prefer key auth and say so when the user offers a password.
- `rm` deletes immediately with no undo: confirm with the user first unless
  they explicitly asked to remove that connection.
- Names must be unique; the command fails with a clear message otherwise.
- The TUI picks up these changes on its own, even while it is open.

After a change, run `show <name>` and report the result.
