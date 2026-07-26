# expense-bot

A single-user Telegram bot in Rust. You send it `4500 cafe con Juan`, it writes a
row to a local SQLite file and replies with a confirmation and your running
monthly total.

It uses long polling, so it only ever makes outbound HTTPS connections to
`api.telegram.org`. No port forwarding, no public URL, no reverse proxy. It ships
as one static binary with no runtime dependencies and runs under systemd.

## Message format

```
<monto> <categoria> [descripcion]
```

```
4500 cafe con Juan
12000 super
1.500 transporte subte
```

Amounts are whole pesos. `.`, `,` and spaces are treated as thousand separators,
so `1.500` is `1500`, and `1,5k` is also `1500`.

Categories are a fixed list: `cafe`, `comida`, `transporte`, `salud`, `super`,
`otros`. Accents and plurals are normalized (`café` and `cafes` both mean
`cafe`). An unrecognized category is rejected with the list of valid ones rather
than quietly filed under "otros", so nothing ends up miscategorized.

When a `comida` expense pushes the Monday-to-Sunday total to 100.000 or more, the
confirmation is followed once by a limit warning, on the expense that crosses the
line.

### Commands

| Command           | What it does                                                       |
| ----------------- | ------------------------------------------------------------------ |
| `/hoy`            | Today's total, broken down by category                              |
| `/semana`         | This week's total by category, with progress against any limit      |
| `/mes [YYYY-MM]`  | Month total by category, current month by default                   |
| `/ultimos [n]`    | The last n entries with their ids, 10 by default                    |
| `/borrar [id]`    | Delete the most recent entry, or a specific one by id               |
| `/export [YYYY-MM]` | Send the month as a CSV file into the chat                        |
| `/ayuda`          | Format reminder and category list                                   |

## What you need from Telegram

Two values, both free and obtained inside Telegram itself:

1. **A bot token.** Message [@BotFather](https://t.me/BotFather), send `/newbot`,
   follow the prompts, and copy the token it gives you. It looks like
   `123456789:AAH_long_random_string`.
2. **Your numeric user id.** Message [@userinfobot](https://t.me/userinfobot) and
   it replies with your id, a number like `987654321`.

The bot checks every incoming message against that id and silently drops
everything else. Anyone can find a public bot by its `@username`, so without the
allow-list a stranger could write into your database.

## Configuration

| Variable          | Required | Default                    | Notes                                    |
| ----------------- | -------- | -------------------------- | ---------------------------------------- |
| `BOT_TOKEN`       | yes      | —                          | From @BotFather                          |
| `ALLOWED_USER_ID` | yes      | —                          | Numeric Telegram user id, from @userinfobot |
| `DB_PATH`         | no       | `expenses.db` (relative)   | Relative to the working directory        |

On the server the unit sets `WorkingDirectory=/var/lib/expense-bot`, so the
default `DB_PATH` resolves to `/var/lib/expense-bot/expenses.db`.

## Local development

Anywhere with a Rust toolchain, including WSL Ubuntu:

```bash
cp .env.example .env    # then fill in BOT_TOKEN and ALLOWED_USER_ID
cargo test
cargo clippy --all-targets
cargo run
```

`.env` is only read in development and is gitignored. Running locally with the
real token is fine as long as the server copy is not running at the same time:
Telegram hands each update to whichever poller asks first, so two instances would
split your messages between them.

## Build

The server is Ubuntu 16.04 with glibc 2.23, so the binary must target
`x86_64-unknown-linux-musl`. A `x86_64-unknown-linux-gnu` binary built on any
current distro fails there with `GLIBC_2.28 not found`. musl links libc
statically, which sidesteps the problem entirely and means there is nothing to
install on the server.

Cross-compiling from Windows is handled by Docker. The `Dockerfile` builds on
`rust:alpine` with `musl-dev` added for `rusqlite`'s bundled SQLite and `ring`'s
assembly, and its final stage is `scratch` containing only the binary. That lets
buildx export straight to disk instead of producing an image.

The build needs **Rust 1.95 or newer**. `ureq` 3.3 only asks for 1.85, but
`rusqlite`'s `bundled` feature pulls in `libsqlite3-sys` 0.38, whose build
script uses the `cfg_select!` macro that stabilized in 1.95. `libsqlite3-sys`
declares no `rust-version`, so Cargo cannot see the constraint and will not pick
an older version to work around it — an older toolchain simply fails to compile.
`rust:alpine` tracks current stable, so it satisfies this; pin to
`rust:1.95-alpine` or newer, never below.

PowerShell:

```powershell
docker buildx build --output "type=local,dest=./dist" .
```

bash (WSL, Linux, macOS):

```bash
docker buildx build --output type=local,dest=./dist .
```

Either way you get `dist/expense-bot`, a ~5 MB static executable.

As an alternative, if you would rather build natively inside WSL Ubuntu:

```bash
rustup target add x86_64-unknown-linux-musl
sudo apt-get install musl-tools
cargo build --release --target x86_64-unknown-linux-musl
# binary at target/x86_64-unknown-linux-musl/release/expense-bot
```

## Deploy

`deploy.sh` and `install.sh` are bash scripts. Run `deploy.sh` from WSL Ubuntu
(it needs `docker`, `scp` and `ssh`; enable Docker Desktop's WSL integration so
the `docker` CLI is on the WSL path). Run `install.sh` on the server itself, as
root. Neither one works in PowerShell — the PowerShell equivalents are spelled
out below if you prefer to stay in Windows.

### First time, on the server

Copy the installer and the unit file over and run it once:

```bash
scp install.sh expense-bot.service me@home-server:/tmp/
ssh me@home-server 'cd /tmp && sudo bash install.sh'
```

It creates the `expense-bot` system user (no home, no login shell), creates
`/var/lib/expense-bot` owned by that user with mode 0750, installs the unit into
`/etc/systemd/system/`, prompts for the bot token and your user id and writes
them to `/etc/expense-bot.env` with mode 0600, then reloads systemd and enables
the service. It does not start it, because the binary is not there yet.

The script is idempotent: re-run it after changing the unit file and it will
update only what changed, and it will not overwrite an existing
`/etc/expense-bot.env` without asking.

### Every time after that, from WSL

```bash
./deploy.sh me@home-server
```

or, with the target in the environment:

```bash
DEPLOY_TARGET=me@home-server ./deploy.sh
```

That builds, copies the binary up, installs it into `/usr/local/bin/`, restarts
the service, and prints the last 20 journal lines so a failed start is visible
immediately. It exits non-zero if the service is not running afterwards. If you
connect to the server as root, set `DEPLOY_SUDO=` to drop the `sudo` prefix.

The same thing by hand, from PowerShell:

```powershell
docker buildx build --output "type=local,dest=./dist" .
scp .\dist\expense-bot me@home-server:/tmp/expense-bot.incoming
ssh me@home-server "sudo install -m755 /tmp/expense-bot.incoming /usr/local/bin/expense-bot && sudo systemctl restart expense-bot"
ssh me@home-server "sudo journalctl -u expense-bot -n 20 --no-pager"
```

### About the systemd unit

Ubuntu 16.04 ships systemd 229, and several of the hardening directives you would
normally reach for are newer than that: `StateDirectory=` needs 235,
`ProtectSystem=strict` and `DynamicUser=` need 232, `ReadWritePaths=` needs 231.
Using them here would be silently ignored or would fail the unit outright, so the
unit sticks to `NoNewPrivileges=yes`, `PrivateTmp=yes`, `ProtectHome=yes` and
`ProtectSystem=full`. `full` leaves `/var` writable, which is what keeps the
database reachable, and the state directory is created by `install.sh` rather
than by systemd.

## The database

One SQLite file at `/var/lib/expense-bot/expenses.db`, owned by the `expense-bot`
user in a directory with mode 0750. It runs in WAL mode with
`synchronous = FULL`, which means that while the service is running there are two
sidecar files next to it:

```
expenses.db
expenses.db-wal
expenses.db-shm
```

**Copying `expenses.db` alone while the bot is running will give you a stale or
corrupt backup**, because recent commits may still live in the `-wal` file. Two
safe options:

Online, with the sqlite3 CLI (`sudo apt-get install sqlite3` if it is missing):

```bash
sudo -u expense-bot sqlite3 /var/lib/expense-bot/expenses.db \
  ".backup '/tmp/expenses-$(date +%F).db'"
```

`.backup` takes a consistent snapshot of a live database, WAL included, and
produces a single self-contained file you can copy off the box.

Offline, no extra tools:

```bash
sudo systemctl stop expense-bot
sudo cp /var/lib/expense-bot/expenses.db /tmp/expenses-$(date +%F).db
sudo systemctl start expense-bot
```

A clean shutdown checkpoints the WAL back into the main file and removes the
sidecars, so the copy is complete on its own.

Restoring is the reverse: stop the service, put the file back as
`/var/lib/expense-bot/expenses.db` owned by `expense-bot:expense-bot`, delete any
leftover `-wal` and `-shm` files, start the service.

## Logs

The bot writes plain text to stdout and stderr, which systemd captures into the
journal.

```bash
sudo journalctl -u expense-bot -f          # follow
sudo journalctl -u expense-bot -n 100      # last 100 lines
sudo journalctl -u expense-bot --since today
sudo systemctl status expense-bot          # is it running, last few lines
```

If the service will not start, the usual causes are a missing or malformed
`/etc/expense-bot.env`, a token that has been revoked in @BotFather, or the
binary not being executable. All three say so in the journal.

## A note on the host

Ubuntu 16.04 is past end of standard support and past the April 2026 ESM cutoff
unless the machine is on Ubuntu Pro. The bot adds little exposure of its own — it
opens no listening port and only makes outbound connections — but the host itself
is unpatched. Worth knowing; not a reason to block the bot.
