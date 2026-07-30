#!/usr/bin/env bash
#
# From-scratch install on a fresh Ubuntu machine. Builds the bot natively,
# installs it as a systemd service, prompts for Telegram credentials, and starts
# it. Run from the repo directory:
#
#   sudo bash setup-ubuntu.sh
#
# Or non-interactively:
#
#   sudo BOT_TOKEN='123:AAH...' ALLOWED_USER_ID='987654321' bash setup-ubuntu.sh
#
# Prerequisites: this script next to Cargo.toml and expense-bot.service (i.e.
# clone the repo first). Needs outbound HTTPS for apt, rustup, crates.io, and
# api.telegram.org.
#
# This targets a modern Ubuntu host that will run the bot itself. For the old
# Ubuntu 16.04 musl deploy path, keep using install.sh + deploy.sh.

set -euo pipefail

SERVICE_NAME=expense-bot
SERVICE_USER=expense-bot
SERVICE_GROUP=expense-bot
STATE_DIR=/var/lib/expense-bot
ENV_FILE=/etc/expense-bot.env
UNIT_DEST=/etc/systemd/system/expense-bot.service
BIN_PATH=/usr/local/bin/expense-bot
MIN_RUST_VERSION=1.95.0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UNIT_SRC="$SCRIPT_DIR/expense-bot.service"
# Build as the invoking user when possible so cargo's cache lives under a real
# home; fall back to root when the script was started with a bare sudo login.
BUILD_USER="${SUDO_USER:-root}"
BUILD_HOME="$(getent passwd "$BUILD_USER" | cut -d: -f6)"
CARGO_BIN="$BUILD_HOME/.cargo/bin"

die() {
	echo "setup-ubuntu.sh: $*" >&2
	exit 1
}

step() {
	echo
	echo "==> $*"
}

version_ge() {
	# Returns 0 when $1 >= $2 (both dotted numeric, e.g. 1.97.1 >= 1.95.0).
	printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1 | grep -qx "$2"
}

as_build_user() {
	if [ "$(id -u)" -eq 0 ] && [ "$BUILD_USER" != root ]; then
		sudo -u "$BUILD_USER" -H env \
			HOME="$BUILD_HOME" \
			PATH="$CARGO_BIN:$PATH" \
			"$@"
	else
		env HOME="$BUILD_HOME" PATH="$CARGO_BIN:$PATH" "$@"
	fi
}

[ "$(id -u)" -eq 0 ] || die "must run as root (try: sudo bash setup-ubuntu.sh)"
[ -f "$SCRIPT_DIR/Cargo.toml" ] || die "Cargo.toml not found in $SCRIPT_DIR; clone the repo and run from there"
[ -f "$UNIT_SRC" ] || die "expense-bot.service not found next to this script"
command -v systemctl >/dev/null || die "systemctl not found; this script targets a systemd Ubuntu host"
command -v apt-get >/dev/null || die "apt-get not found; this script targets Ubuntu/Debian"

step "System packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y --no-install-recommends \
	build-essential \
	ca-certificates \
	curl \
	pkg-config
echo "build tools installed"

step "Rust toolchain (needs >= $MIN_RUST_VERSION)"
if as_build_user bash -lc 'command -v rustc >/dev/null && command -v cargo >/dev/null'; then
	echo "rustc already present for $BUILD_USER"
else
	echo "installing rustup for $BUILD_USER"
	as_build_user bash -lc \
		"curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable"
fi

# Ensure cargo is on PATH for subsequent as_build_user calls even when the
# user's shell rc has not been sourced yet.
[ -x "$CARGO_BIN/rustc" ] || die "rustc missing after rustup install at $CARGO_BIN"
[ -x "$CARGO_BIN/cargo" ] || die "cargo missing after rustup install at $CARGO_BIN"

as_build_user "$CARGO_BIN/rustup" update stable
as_build_user "$CARGO_BIN/rustup" default stable

RUSTC_VERSION="$(as_build_user "$CARGO_BIN/rustc" --version | awk '{print $2}')"
version_ge "$RUSTC_VERSION" "$MIN_RUST_VERSION" \
	|| die "rustc $RUSTC_VERSION is too old; need >= $MIN_RUST_VERSION (libsqlite3-sys needs cfg_select!)"
echo "using rustc $RUSTC_VERSION"

step "Building release binary"
as_build_user "$CARGO_BIN/cargo" build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"
RELEASE_BIN="$SCRIPT_DIR/target/release/expense-bot"
[ -x "$RELEASE_BIN" ] || die "build finished but $RELEASE_BIN is missing"
echo "built $RELEASE_BIN ($(du -h "$RELEASE_BIN" | cut -f1))"

step "Service account"
if getent group "$SERVICE_GROUP" >/dev/null; then
	echo "group $SERVICE_GROUP already exists"
else
	groupadd --system "$SERVICE_GROUP"
	echo "created system group $SERVICE_GROUP"
fi

if getent passwd "$SERVICE_USER" >/dev/null; then
	echo "user $SERVICE_USER already exists"
else
	useradd --system \
		--gid "$SERVICE_GROUP" \
		--home-dir /nonexistent \
		--no-create-home \
		--shell /usr/sbin/nologin \
		"$SERVICE_USER"
	echo "created system user $SERVICE_USER"
fi

step "State directory"
install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0750 "$STATE_DIR"
echo "$STATE_DIR ready (database will be $STATE_DIR/expenses.db)"

step "Installing binary"
install -o root -g root -m 755 "$RELEASE_BIN" "$BIN_PATH.new"
mv -f "$BIN_PATH.new" "$BIN_PATH"
echo "installed $BIN_PATH"

step "systemd unit"
install -o root -g root -m 0644 "$UNIT_SRC" "$UNIT_DEST"
echo "installed $UNIT_DEST"

step "Environment file"
write_env=yes
if [ -e "$ENV_FILE" ]; then
	echo "$ENV_FILE already exists."
	if [ -n "${BOT_TOKEN:-}" ] && [ -n "${ALLOWED_USER_ID:-}" ]; then
		write_env=yes
		echo "BOT_TOKEN and ALLOWED_USER_ID provided in the environment; overwriting"
	elif [ -t 0 ]; then
		printf 'Overwrite it with new values? [y/N] '
		read -r answer
		case "$answer" in
		[yY] | [yY][eE][sS]) write_env=yes ;;
		*) write_env=no ;;
		esac
	else
		write_env=no
		echo "keeping the existing environment file unchanged"
	fi
fi

if [ "$write_env" = yes ]; then
	bot_token="${BOT_TOKEN:-}"
	allowed_user_id="${ALLOWED_USER_ID:-}"

	if [ -z "$bot_token" ] || [ -z "$allowed_user_id" ]; then
		[ -t 0 ] || die "no terminal and no BOT_TOKEN/ALLOWED_USER_ID env vars; cannot write $ENV_FILE"
		echo "BOT_TOKEN comes from @BotFather, ALLOWED_USER_ID from @userinfobot."
	fi

	while [ -z "$bot_token" ]; do
		printf 'BOT_TOKEN (input hidden): '
		read -r -s bot_token
		echo
		if ! printf '%s' "$bot_token" | grep -Eq '^[0-9]+:[A-Za-z0-9_-]{30,}$'; then
			echo "that does not look like a bot token (expected 123456789:AA...); try again" >&2
			bot_token=""
		fi
	done

	while [ -z "$allowed_user_id" ]; do
		printf 'ALLOWED_USER_ID (numeric): '
		read -r allowed_user_id
		if ! printf '%s' "$allowed_user_id" | grep -Eq '^[0-9]+$'; then
			echo "the Telegram user id is digits only; try again" >&2
			allowed_user_id=""
		fi
	done

	(
		umask 077
		cat >"$ENV_FILE" <<EOF
# Written by setup-ubuntu.sh. Read by systemd, not by a shell: no quoting, no export.
BOT_TOKEN=$bot_token
ALLOWED_USER_ID=$allowed_user_id
# DB_PATH defaults to expenses.db under WorkingDirectory ($STATE_DIR).
EOF
	)
	chown "$SERVICE_USER:$SERVICE_GROUP" "$ENV_FILE"
	chmod 0600 "$ENV_FILE"
	echo "wrote $ENV_FILE (mode 0600)"
fi

step "Enable and start $SERVICE_NAME"
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
systemctl restart "$SERVICE_NAME"
sleep 2

active="$(systemctl is-active "$SERVICE_NAME" || true)"
echo "$SERVICE_NAME is $active"
echo
journalctl -u "$SERVICE_NAME" -n 20 --no-pager || true

if [ "$active" != "active" ]; then
	echo
	die "$SERVICE_NAME failed to stay running; follow logs with: journalctl -u $SERVICE_NAME -f"
fi

echo
echo "Done. The bot is installed and running."
echo "  status:  systemctl status $SERVICE_NAME"
echo "  logs:    journalctl -u $SERVICE_NAME -f"
echo "  restart: systemctl restart $SERVICE_NAME"
echo "  env:     $ENV_FILE"
echo "  db:      $STATE_DIR/expenses.db"
