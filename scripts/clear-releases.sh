#!/usr/bin/env bash
# Delete GitHub releases (and their assets) while keeping the git tags.
#
# Usage:
#   scripts/clear-releases.sh                 # delete ALL releases
#   scripts/clear-releases.sh v1.2.3          # delete a single release by tag
#   scripts/clear-releases.sh --dry-run       # list what would be deleted
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
for arg in "$@"; do
  case "$arg" in
    --dry-run|-n) DRY_RUN=true ;;
    *) TARGET="$arg" ;;
  esac
done

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
  # All releases
  TAGS="$(gh release list --repo "$REPO" --limit 1000 --json tagName -q '.[].tagName' 2>/dev/null || true)"
  if [[ -z "$TAGS" ]]; then
    echo "No releases found."
    exit 0
  fi

  while IFS= read -r tag; do
    if $DRY_RUN; then
      echo "Would delete release: $tag"
    else
      echo "Deleting release: $tag"
      gh release delete "$tag" --repo "$REPO" --yes
    fi
  done <<< "$TAGS"
fi

echo "Done."
