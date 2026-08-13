#!/usr/bin/env bash
# Delete GitHub releases (and their assets) while keeping the git tags.
#
# Usage:
#   scripts/clear-releases.sh                     # delete ALL releases
#   scripts/clear-releases.sh v1.2.3              # delete a single release by tag
#   scripts/clear-releases.sh --below 1.14        # delete releases older than 1.14
#   scripts/clear-releases.sh --dry-run           # list what would be deleted
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh (GitHub CLI) is required." >&2
  exit 1
fi

REPO="${GH_REPO:-$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null || true)}"
if [[ -z "$REPO" ]]; then
  echo "error: could not determine repository. Run inside the repo or set GH_REPO." >&2
  exit 1
fi

DRY_RUN=false
TARGET=""
BELOW=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run|-n) DRY_RUN=true ;;
    --below)
      BELOW="$2"
      shift
      ;;
    *) TARGET="$1" ;;
  esac
  shift
done

# Convert a version like "v1.13.2" or "1.13.2" into a sortable zero-padded form.
version_key() {
  local v="$1"
  v="${v#v}"
  echo "$v" | awk -F. '{ printf "%04d.%04d.%04d", $1, $2, $3 }'
}

if [[ -n "$TARGET" ]]; then
  # Single release by tag
  if gh release view "$TARGET" --repo "$REPO" &>/dev/null; then
    if $DRY_RUN; then
      echo "Would delete release: $TARGET"
    else
      echo "Deleting release: $TARGET"
      gh release delete "$TARGET" --repo "$REPO" --yes
    fi
  else
    echo "Release $TARGET not found." >&2
    exit 1
  fi
else
  # List releases
  TAGS="$(gh release list --repo "$REPO" --limit 1000 --json tagName -q '.[].tagName' 2>/dev/null || true)"
  if [[ -z "$TAGS" ]]; then
    echo "No releases found."
    exit 0
  fi

  if [[ -n "$BELOW" ]]; then
    BELOW_KEY="$(version_key "$BELOW")"
  fi

  while IFS= read -r tag; do
    [[ -z "$tag" ]] && continue

    if [[ -n "$BELOW" ]]; then
      TAG_KEY="$(version_key "$tag")"
      # Skip if tag version is >= below threshold (keep newer releases)
      if [[ "$TAG_KEY" > "$BELOW_KEY" || "$TAG_KEY" == "$BELOW_KEY" ]]; then
        continue
      fi
    fi

    if $DRY_RUN; then
      echo "Would delete release: $tag"
    else
      echo "Deleting release: $tag"
      gh release delete "$tag" --repo "$REPO" --yes
    fi
  done <<< "$TAGS"
fi

echo "Done."
