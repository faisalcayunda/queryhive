#!/usr/bin/env bash
# Shared venv bootstrap for run_local.sh and build_dmg.sh.
#
# PYTHON_TAG is free-threaded on purpose. Measured on 200k rows, 6 columns:
# overlapping the Trino fetch with the writer is worth 1.3x-1.5x on any build,
# but rendering rows across several threads only pays off without the GIL
# (2.8x vs 1.3x when the coordinator serves pages in ~5ms). Set PYTHON_TAG=3.14
# to fall back to the GIL build; everything still works.
PYTHON_TAG="${PYTHON_TAG:-3.14t}"

# "3.14t" for a free-threaded build, "3.12" for a stock one.
_venv_tag() {
  "$1" -c 'import sys,sysconfig;print("%d.%d%s"%(*sys.version_info[:2],"t" if sysconfig.get_config_var("Py_GIL_DISABLED") else ""))' 2>/dev/null
}

ensure_venv() {
  uv python find "$PYTHON_TAG" >/dev/null 2>&1 ||
    { echo "python $PYTHON_TAG missing; run: uv python install $PYTHON_TAG" >&2; exit 1; }

  # Recreate when the venv was built against a different interpreter, otherwise
  # changing PYTHON_TAG silently keeps the old one.
  if [[ -x .venv/bin/python && "$(_venv_tag .venv/bin/python)" != "$PYTHON_TAG" ]]; then
    echo "==> venv is $(_venv_tag .venv/bin/python), rebuilding for $PYTHON_TAG"
    rm -rf .venv
  fi
  [[ -d .venv ]] || uv venv --python "$PYTHON_TAG" .venv

  VIRTUAL_ENV=.venv uv pip install -q -r requirements.txt
  # --no-deps skips orjson, which refuses to build on free-threaded Python.
  VIRTUAL_ENV=.venv uv pip install -q --no-deps -r requirements-trino.txt
}
