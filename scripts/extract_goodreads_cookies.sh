#!/usr/bin/env bash
set -euo pipefail

OUTPUT="goodreads_config.json"
USER_ID=""

usage() {
    echo "Usage: $0 [--output FILE] [--user-id ID]"
    echo ""
    echo "Extract Goodreads cookies from Firefox and write a config file."
    echo ""
    echo "Options:"
    echo "  --output FILE   Output config file path (default: goodreads_config.json)"
    echo "  --user-id ID    Your Goodreads user ID (found in the URL on goodreads.com/review/import)"
    echo "  --help          Show this help"
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --output)
            OUTPUT="$2"
            shift 2
            ;;
        --user-id)
            USER_ID="$2"
            shift 2
            ;;
        --help)
            usage
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage
            ;;
    esac
done

# Find all cookies.sqlite files under the Firefox directory
FIREFOX_DIR="$HOME/Library/Application Support/Firefox"
if [[ ! -d "$FIREFOX_DIR" ]]; then
    echo "Error: Firefox directory not found at $FIREFOX_DIR" >&2
    exit 1
fi

COOKIE_DBS=()
while IFS= read -r line; do
    COOKIE_DBS+=("$line")
done < <(find "$FIREFOX_DIR" -name "cookies.sqlite" -type f 2>/dev/null)

if [[ ${#COOKIE_DBS[@]} -eq 0 ]]; then
    echo "Error: No cookies.sqlite found under $FIREFOX_DIR" >&2
    exit 1
elif [[ ${#COOKIE_DBS[@]} -eq 1 ]]; then
    COOKIES_DB="${COOKIE_DBS[0]}"
    echo "Found cookies DB: $COOKIES_DB"
else
    echo "Found multiple cookies.sqlite files:"
    for i in "${!COOKIE_DBS[@]}"; do
        echo "  [$i] ${COOKIE_DBS[$i]}"
    done
    read -rp "Pick one [0-$((${#COOKIE_DBS[@]} - 1))]: " CHOICE
    COOKIES_DB="${COOKIE_DBS[$CHOICE]}"
fi

# Copy the DB since Firefox holds a lock on it while running
TMPDB=$(mktemp /tmp/firefox_cookies.XXXXXX)
trap 'rm -f "$TMPDB"' EXIT
cp "$COOKIES_DB" "$TMPDB"

# Extract cookies for .goodreads.com and www.goodreads.com (skip other subdomains like help.*)
COOKIE_STRING=$(sqlite3 "$TMPDB" \
    "SELECT name || '=' || value FROM moz_cookies WHERE host IN ('.goodreads.com', 'www.goodreads.com') ORDER BY name;" \
    | awk '{printf "%s%s", sep, $0; sep="; "}')

if [[ -z "$COOKIE_STRING" ]]; then
    echo "Error: No Goodreads cookies found. Make sure you're logged in to goodreads.com in Firefox." >&2
    exit 1
fi

echo "Found $(echo "$COOKIE_STRING" | tr ';' '\n' | wc -l | tr -d ' ') Goodreads cookies"

# Try to extract user_id from cookies if not provided
if [[ -z "$USER_ID" ]]; then
    # The 'u' cookie sometimes contains the user ID
    CANDIDATE=$(sqlite3 "$TMPDB" \
        "SELECT value FROM moz_cookies WHERE host LIKE '%goodreads.com' AND name = 'u' LIMIT 1;" 2>/dev/null || true)
    if [[ -n "$CANDIDATE" ]]; then
        USER_ID="$CANDIDATE"
        echo "Extracted user_id from cookie: $USER_ID"
    fi
fi

if [[ -z "$USER_ID" ]]; then
    echo ""
    echo "Could not automatically determine your Goodreads user ID."
    echo "You can find it by visiting https://www.goodreads.com/review/import"
    echo "and looking at the export URL which contains your numeric user ID."
    echo ""
    read -rp "Enter your Goodreads user ID: " USER_ID
fi

if [[ -z "$USER_ID" ]]; then
    echo "Error: user_id is required" >&2
    exit 1
fi

# Write config JSON (use python to properly escape values)
python3 -c "
import json, sys
json.dump({'user_id': sys.argv[1], 'cookies': sys.argv[2]}, open(sys.argv[3], 'w'), indent=2)
" "$USER_ID" "$COOKIE_STRING" "$OUTPUT"

echo "Config written to $OUTPUT"
