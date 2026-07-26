#!/usr/bin/env bash
#
# One-time server setup for expense-bot. Runs on the Ubuntu 16.04 box as root:
#
#   scp install.sh expense-bot.service server:/tmp/
#   ssh server 'cd /tmp && sudo bash install.sh'
#
# Creates the service account and state directory, installs the systemd unit,
# writes /etc/expense-bot.env, then enables the service. It does not install the
# binary and does not start the service - deploy.sh does both.
#
# Safe to re-run: every step is a no-op when it is already done, and an existing
# environment file is never overwritten without asking.

set -euo pipefail

SERVICE_NAME=expense-bot
SERVICE_USER=expense-bot
SERVICE_GROUP=expense-bot
STATE_DIR=/var/lib/expense-bot
ENV_FILE=/etc/expense-bot.env
UNIT_SRC_NAME=expense-bot.service
UNIT_DEST=/etc/systemd/system/expense-bot.service
BIN_PATH=/usr/local/bin/expense-bot

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UNIT_SRC="$SCRIPT_DIR/$UNIT_SRC_NAME"

die() {
	echo "install.sh: $*" >&2
	exit 1
}

step() {
	echo
	echo "==> $*"
}

[ "$(id -u)" -eq 0 ] || die "must run as root (try: sudo bash install.sh)"
[ -f "$UNIT_SRC" ] || die "$UNIT_SRC_NAME not found next to this script in $SCRIPT_DIR"
command -v systemctl >/dev/null || die "systemctl not found; this script targets a systemd host"

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
	echo "created system user $SERVICE_USER (no home, no login shell)"
fi

step "State directory"
# install -d is idempotent and also corrects mode and ownership on an existing
# directory. 0750 keeps the database readable only by the service and root.
install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0750 "$STATE_DIR"
echo "$STATE_DIR ready (owner $SERVICE_USER:$SERVICE_GROUP, mode 0750)"
echo "the database will be $STATE_DIR/expenses.db"

step "systemd unit"
if [ -f "$UNIT_DEST" ] && cmp -s "$UNIT_SRC" "$UNIT_DEST"; then
	echo "$UNIT_DEST is already up to date"
else
	install -o root -g root -m 0644 "$UNIT_SRC" "$UNIT_DEST"
	echo "installed $UNIT_DEST"
fi

step "Environment file"
write_env=yes
if [ -e "$ENV_FILE" ]; then
	echo "$ENV_FILE already exists."
	if [ -t 0 ]; then
		printf 'Overwrite it with new values? [y/N] '
		read -r answer
		case "$answer" in
		[yY] | [yY][eE][sS]) write_env=yes ;;
		*) write_env=no ;;
		esac
	else
		write_env=no
	fi
	[ "$write_env" = yes ] || echo "keeping the existing environment file unchanged"
fi

if [ "$write_env" = yes ]; then
	[ -t 0 ] || die "no terminal to prompt on; create $ENV_FILE by hand (BOT_TOKEN, ALLOWED_USER_ID) and re-run"

	echo "BOT_TOKEN comes from @BotFather, ALLOWED_USER_ID from @userinfobot."
	bot_token=""
	while [ -z "$bot_token" ]; do
		printf 'BOT_TOKEN (input hidden): '
		read -r -s bot_token
		echo
		if ! printf '%s' "$bot_token" | grep -Eq '^[0-9]+:[A-Za-z0-9_-]{30,}$'; then
			echo "that does not look like a bot token (expected 123456789:AA...); try again" >&2
			bot_token=""
		fi
	done

	allowed_user_id=""
	while [ -z "$allowed_user_id" ]; do
		printf 'ALLOWED_USER_ID (numeric): '
		read -r allowed_user_id
		if ! printf '%s' "$allowed_user_id" | grep -Eq '^[0-9]+$'; then
			echo "the Telegram user id is digits only; try again" >&2
			allowed_user_id=""
		fi
	done

	# umask before the redirection so the token is never briefly world-readable.
	(
		umask 077
		cat >"$ENV_FILE" <<EOF
# Written by install.sh. Read by systemd, not by a shell: no quoting, no export.
BOT_TOKEN=$bot_token
ALLOWED_USER_ID=$allowed_user_id
# DB_PATH is optional and defaults to expenses.db relative to WorkingDirectory,
# which the unit sets to $STATE_DIR.
# DB_PATH=$STATE_DIR/expenses.db
EOF
	)
	chown "$SERVICE_USER:$SERVICE_GROUP" "$ENV_FILE"
	chmod 0600 "$ENV_FILE"
	echo "wrote $ENV_FILE (owner $SERVICE_USER:$SERVICE_GROUP, mode 0600)"
fi

step "Enabling the service"
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
echo "$SERVICE_NAME will start on boot"

echo
if [ -x "$BIN_PATH" ]; then
	echo "Setup complete. $BIN_PATH is already installed; start or restart it with:"
	echo "  sudo systemctl restart $SERVICE_NAME"
else
	echo "Setup complete. The binary is not installed yet, so the service is enabled"
	echo "but not started. From your development machine run:"
	echo "  ./deploy.sh <ssh-target>"
fi
echo "Logs: sudo journalctl -u $SERVICE_NAME -f"
