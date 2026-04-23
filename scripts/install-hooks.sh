#!/usr/bin/env bash
# Wire the shared .githooks/ directory into the local git config.
# Run once after cloning:  bash scripts/install-hooks.sh
set -e
git config core.hooksPath .githooks
chmod +x .githooks/pre-push
echo "hooks installed — .githooks/pre-push will run on every git push"
