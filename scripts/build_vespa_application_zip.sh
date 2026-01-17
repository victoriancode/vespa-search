#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_dir="${repo_root}/vespa/application"
output_zip="${repo_root}/vespa-application.zip"

if [[ ! -d "${app_dir}" ]]; then
  echo "Vespa application directory not found: ${app_dir}" >&2
  exit 1
fi

(cd "${app_dir}" && zip -r "${output_zip}" .)
echo "Wrote ${output_zip}"
