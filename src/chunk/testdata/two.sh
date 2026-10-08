#!/usr/bin/env bash
set -euo pipefail

# First.
one() {
    local y=$(( $1 + 1 ))
    echo $(( y * 2 ))
}

# Second.
two() {
    local total=0
    for item in "$@"; do
        total=$(( total + item ))
    done
    echo "$total"
}
