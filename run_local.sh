#!/usr/bin/env bash
# Start the local web UI: ./run_local.sh  ->  http://127.0.0.1:8765
set -euo pipefail
cd "$(dirname "$0")"
source ./venv_setup.sh
ensure_venv
exec .venv/bin/python app.py "$@"
