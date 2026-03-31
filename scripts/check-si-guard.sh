#!/usr/bin/env bash
set -euo pipefail

# Enforce that imperial->SI conversion markers do not spread in equipment runtime code.
cargo test -p hares-equipment --test si_guard
