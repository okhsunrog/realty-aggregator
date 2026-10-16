#!/usr/bin/env bash
set -euo pipefail
# Load only the model credential from the user's shared environment file.
if [[ -z "${MY_AI_PROXY:-}" && -r "$HOME/.config/environment.d/60-secrets.conf" ]]; then
    MY_AI_PROXY=$(
        set +u
        source "$HOME/.config/environment.d/60-secrets.conf" >/dev/null 2>&1
        printf '%s' "${MY_AI_PROXY:-}"
    )
    export MY_AI_PROXY
fi
binary_dir=${REALTY_BIN_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/realty-aggregator/bin}
exec "$binary_dir/backend" "$@"
