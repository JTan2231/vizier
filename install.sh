#!/bin/zsh
set -euo pipefail

cargo build --release

sudo cp target/release/vizier /usr/local/bin/
sudo mkdir -p /usr/local/share/vizier/agents
sudo cp examples/agents/*.sh /usr/local/share/vizier/agents/
sudo chmod +x /usr/local/share/vizier/agents/*.sh
