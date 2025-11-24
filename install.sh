#!/bin/zsh
set -euo pipefail

cargo build --release

sudo cp target/release/vizier /usr/local/bin/
sudo mkdir -p /usr/local/share/vizier/agents
sudo cp -R examples/agents/* /usr/local/share/vizier/agents/
sudo find /usr/local/share/vizier/agents -type f -name "*.sh" -exec chmod +x {} +
