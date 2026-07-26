#!/usr/bin/env bash
#
# Build the static Linux binary and ship it to the server. Run from WSL Ubuntu
# (or any Linux/macOS box with docker, scp and ssh):
#
#   ./deploy.sh me@home-server
#   DEPLOY_TARGET=me@home-server ./deploy.sh
#
# The ssh target is whatever ssh understands: user@host, a host, or an alias
# from ~/.ssh/config. Run install.sh on the server once before the first deploy.
#
# Environment:
#   DEPLOY_TARGET   ssh target, used when no argument is given
#   DEPLOY_SUDO     command used to elevate on the server; set to "" when you
#                   connect as root (default: sudo)

set -euo pipefail

SERVICE_NAME=expense-bot
BIN_NAME=expense-bot
BIN_PATH=/usr/local/bin/expense-bot
REMOTE_TMP=/tmp/expense-bot.incoming
JOURNAL_LINES=20

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="$SCRIPT_DIR/dist"

SSH_TARGET="${1:-${DEPLOY_TARGET:-}}"
SUDO="${DEPLOY_SUDO-sudo}"

die() {
	echo "deploy.sh: $*" >&2
	exit 1
}

step() {
	echo
	echo "==> $*"
}

if [ -z "$SSH_TARGET" ]; then
	echo "usage: $0 <ssh-target>    (or set DEPLOY_TARGET)" >&2
	echo "example: $0 me@home-server" >&2
	exit 1
fi

command -v docker >/dev/null || die "docker not found; on WSL enable Docker Desktop's WSL integration"
command -v ssh >/dev/null || die "ssh not found"
command -v scp >/dev/null || die "scp not found"

step "Building the static musl binary"
# The Dockerfile's final stage is scratch and holds only the binary, so a local
# output export writes exactly one file: dist/expense-bot.
docker buildx build --output "type=local,dest=$DIST_DIR" "$SCRIPT_DIR"

[ -f "$DIST_DIR/$BIN_NAME" ] || die "build finished but $DIST_DIR/$BIN_NAME is missing"
echo "built $DIST_DIR/$BIN_NAME ($(du -h "$DIST_DIR/$BIN_NAME" | cut -f1))"

step "Uploading to $SSH_TARGET"
scp "$DIST_DIR/$BIN_NAME" "$SSH_TARGET:$REMOTE_TMP"

step "Installing and restarting $SERVICE_NAME"
# Installed under a temporary name and then renamed: writing directly over a
# running executable can fail with ETXTBSY, while rename(2) is atomic and the
# running process simply keeps its old inode until systemd restarts it.
ssh "$SSH_TARGET" "set -e
	$SUDO install -o root -g root -m 755 '$REMOTE_TMP' '$BIN_PATH.new'
	$SUDO mv -f '$BIN_PATH.new' '$BIN_PATH'
	rm -f '$REMOTE_TMP'
	$SUDO systemctl restart '$SERVICE_NAME'"

# Give a crashing bot time to crash before reporting on it. Restart=always with
# RestartSec=5 means a broken binary shows up as activating/failed, not running.
sleep 3

step "Status"
active="$(ssh "$SSH_TARGET" "systemctl is-active '$SERVICE_NAME' || true")"
echo "$SERVICE_NAME is $active"

step "Last $JOURNAL_LINES journal lines"
ssh "$SSH_TARGET" "$SUDO journalctl -u '$SERVICE_NAME' -n $JOURNAL_LINES --no-pager" || true

if [ "$active" != "active" ]; then
	echo
	die "$SERVICE_NAME is not running (see the journal above); follow it with: ssh $SSH_TARGET '$SUDO journalctl -u $SERVICE_NAME -f'"
fi

echo
echo "Deployed. Follow the logs with:"
echo "  ssh $SSH_TARGET '$SUDO journalctl -u $SERVICE_NAME -f'"
