#!/usr/bin/env bash
# Runs .github/workflows/prebuilt.yml on GitHub for the current branch,
# waits for it and downloads the libraries into native/prebuilt/ (replacing
# what is there). The branch must be pushed first.
#
# Usage: tool/ci_prebuilt.sh [run-id]   (with a run id: only download)
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
run="${1:-}"
if [[ -z "$run" ]]; then
  branch="$(git branch --show-current)"
  started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  gh workflow run prebuilt.yml --ref "$branch"
  echo "Waiting for the run to appear..."
  for _ in $(seq 1 30); do
    run="$(gh run list --workflow prebuilt.yml --branch "$branch" \
      --event workflow_dispatch --limit 1 \
      --json databaseId,createdAt \
      --jq ".[] | select(.createdAt >= \"$started\") | .databaseId")"
    [[ -n "$run" ]] && break
    sleep 2
  done
  [[ -n "$run" ]] || { echo "the run did not start" >&2; exit 1; }
  gh run watch "$run" --exit-status
fi
rm -rf native/prebuilt
gh run download "$run" --name prebuilt --dir native/prebuilt
cat native/prebuilt/checksums.json
