#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

ARTIFACT_DIR="${1:-${TERMLM_SOAK_ARTIFACT_DIR:-/tmp/termlm-soak-24h-$(date +%Y%m%d-%H%M%S)}}"
DURATION_SECS="${TERMLM_SOAK_DURATION_SECS:-86400}"
PARALLEL_CLIENTS="${TERMLM_SOAK_PARALLEL_CLIENTS:-3}"
PATH_CHURN_WINDOW="${TERMLM_SOAK_PATH_CHURN_WINDOW:-8}"

mkdir -p "${ARTIFACT_DIR}"
START_UTC="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

printf '[soak-24h] artifact dir: %s\n' "${ARTIFACT_DIR}"
printf '[soak-24h] start: %s\n' "${START_UTC}"
printf '[soak-24h] duration target: %ss\n' "${DURATION_SECS}"
printf '[soak-24h] parallel clients: %s\n' "${PARALLEL_CLIENTS}"
printf '[soak-24h] path churn window: %s\n' "${PATH_CHURN_WINDOW}"

METRICS_PATH="${ARTIFACT_DIR}/soak-metrics.json"

(
  cd "${ROOT_DIR}" && \
    TERMLM_SOAK_DURATION_SECS="${DURATION_SECS}" \
    TERMLM_SOAK_PARALLEL_CLIENTS="${PARALLEL_CLIENTS}" \
    TERMLM_SOAK_PATH_CHURN_WINDOW="${PATH_CHURN_WINDOW}" \
    TERMLM_SOAK_METRICS_PATH="${METRICS_PATH}" \
    bash tests/reliability/reliability_drills.sh
)

END_UTC="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
cat > "${ARTIFACT_DIR}/run-meta.json" <<EOF
{
  "start_utc": "${START_UTC}",
  "end_utc": "${END_UTC}",
  "duration_target_secs": ${DURATION_SECS},
  "parallel_clients": ${PARALLEL_CLIENTS},
  "path_churn_window": ${PATH_CHURN_WINDOW},
  "metrics_file": "soak-metrics.json"
}
EOF

printf '[soak-24h] end: %s\n' "${END_UTC}"
printf '[soak-24h] metrics: %s\n' "${METRICS_PATH}"
