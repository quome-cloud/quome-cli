#!/bin/bash
# Setup script for quome-cli development
# Run this after cloning the repository

set -e

GREEN='\033[0;32m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

echo "Installing git hooks..."
cp "$SCRIPT_DIR/hooks/pre-commit" "$ROOT_DIR/.git/hooks/pre-commit"
chmod +x "$ROOT_DIR/.git/hooks/pre-commit"

echo -e "${GREEN}Setup complete!${NC}"
