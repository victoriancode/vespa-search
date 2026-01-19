#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  ./scripts/quick_commit_push.sh -m "commit message" -- <paths...>
  ./scripts/quick_commit_push.sh -m "commit message" --all

Notes:
  - Use --all to stage all changes (tracked + untracked).
  - Use -- <paths...> to stage only specific files.
USAGE
}

commit_message=""
mode=""
paths=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    -m|--message)
      commit_message="${2:-}"
      shift 2
      ;;
    --all|-a)
      mode="all"
      shift
      ;;
    --)
      shift
      paths=("$@")
      break
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ -z "$commit_message" ]]; then
  echo "Missing commit message." >&2
  usage
  exit 2
fi

if [[ "$mode" == "all" ]]; then
  git add -A
else
  if [[ ${#paths[@]} -eq 0 ]]; then
    echo "No paths provided to stage." >&2
    usage
    exit 2
  fi
  git add -- "${paths[@]}"
fi

if git diff --cached --quiet; then
  echo "No staged changes to commit." >&2
  exit 1
fi

git commit -m "$commit_message"
git push
