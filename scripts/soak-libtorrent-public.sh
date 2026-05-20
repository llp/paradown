#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/soak-libtorrent-public.sh --matrix-file <file> [options]

Runs authorized public libtorrent smoke cases from a tab-separated matrix.
Timeouts are treated as diagnostic outcomes by default; pass --require-complete
when every case must finish with exit code 0.

Matrix columns:
  name<TAB>locator<TAB>tracker_file<TAB>tracker_list_url<TAB>discovery_file<TAB>discovery_url<TAB>timeout_secs<TAB>max_trackers<TAB>index_url_template<TAB>index_query<TAB>index_kind<TAB>index_cache_ttl_secs<TAB>index_timeout_secs

Use "-" for blank optional fields. Lines beginning with "#" are ignored.

Options:
  --matrix-file <file>   TSV matrix of authorized public test cases
  --out-dir <dir>        Write downloads, logs, and summary here
  --default-timeout <n>  Timeout seconds when a row omits timeout_secs
  --require-complete     Treat exit code 124 as failure instead of diagnostic timeout
  --fail-fast            Stop after the first failed case
  --dry-run              Print commands without running them
  -h, --help             Show this help message
EOF
}

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SMOKE_SCRIPT="$ROOT_DIR/scripts/smoke-libtorrent-public.sh"
MATRIX_FILE=""
OUT_DIR="$ROOT_DIR/target/libtorrent-public-soak"
DEFAULT_TIMEOUT=180
REQUIRE_COMPLETE=0
FAIL_FAST=0
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --matrix-file)
      MATRIX_FILE="${2:-}"
      if [[ -z "$MATRIX_FILE" ]]; then
        echo "error: --matrix-file requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --out-dir)
      OUT_DIR="${2:-}"
      if [[ -z "$OUT_DIR" ]]; then
        echo "error: --out-dir requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --default-timeout)
      DEFAULT_TIMEOUT="${2:-}"
      if [[ -z "$DEFAULT_TIMEOUT" ]]; then
        echo "error: --default-timeout requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --require-complete)
      REQUIRE_COMPLETE=1
      shift
      ;;
    --fail-fast)
      FAIL_FAST=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option '$1'" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$MATRIX_FILE" ]]; then
  echo "error: --matrix-file is required" >&2
  usage >&2
  exit 1
fi

if [[ ! -f "$MATRIX_FILE" ]]; then
  echo "error: matrix file '$MATRIX_FILE' not found" >&2
  exit 1
fi

if [[ ! -x "$SMOKE_SCRIPT" ]]; then
  echo "error: smoke script '$SMOKE_SCRIPT' is not executable" >&2
  exit 1
fi

if ! [[ "$DEFAULT_TIMEOUT" =~ ^[0-9]+$ ]] || [[ "$DEFAULT_TIMEOUT" -eq 0 ]]; then
  echo "error: --default-timeout must be a positive integer" >&2
  exit 1
fi

mkdir -p "$OUT_DIR/logs" "$OUT_DIR/cases"
SUMMARY="$OUT_DIR/summary.tsv"
printf "name\tstatus\texit_code\tduration_secs\tlog\tcase_dir\n" > "$SUMMARY"

case_count=0
failed_count=0

field_or_empty() {
  local value="${1:-}"
  if [[ "$value" == "-" ]]; then
    printf ''
  else
    printf '%s' "$value"
  fi
}

sanitize_name() {
  local value="$1"
  value="${value//[^A-Za-z0-9._-]/_}"
  if [[ -z "$value" ]]; then
    value="case"
  fi
  printf '%s' "$value"
}

validate_positive_integer() {
  local label="$1"
  local value="$2"
  if ! [[ "$value" =~ ^[0-9]+$ ]] || [[ "$value" -eq 0 ]]; then
    echo "error: $label must be a positive integer" >&2
    exit 1
  fi
}

run_case() {
  local name="$1"
  local locator="$2"
  local tracker_file="$3"
  local tracker_list_url="$4"
  local discovery_file="$5"
  local discovery_url="$6"
  local timeout_secs="$7"
  local max_trackers="$8"
  local index_url_template="$9"
  local index_query="${10}"
  local index_kind="${11}"
  local index_cache_ttl_secs="${12}"
  local index_timeout_secs="${13}"

  local safe_name
  safe_name="$(printf '%03d-%s' "$case_count" "$(sanitize_name "$name")")"
  local case_dir="$OUT_DIR/cases/$safe_name"
  local log_path="$OUT_DIR/logs/${safe_name}.log"
  mkdir -p "$case_dir"

  local cmd=(
    "$SMOKE_SCRIPT"
    --out-dir "$case_dir"
    --timeout "${timeout_secs:-$DEFAULT_TIMEOUT}"
    --provider-cache-dir "$OUT_DIR/provider-cache"
  )

  if [[ -n "$locator" ]]; then
    cmd+=(--locator "$locator")
  fi
  if [[ -n "$tracker_file" ]]; then
    cmd+=(--tracker-file "$tracker_file")
  fi
  if [[ -n "$tracker_list_url" ]]; then
    cmd+=(--tracker-list-url "$tracker_list_url")
  fi
  if [[ -n "$discovery_file" ]]; then
    cmd+=(--discover-file "$discovery_file")
  fi
  if [[ -n "$discovery_url" ]]; then
    cmd+=(--discover-url "$discovery_url")
  fi
  if [[ -n "$max_trackers" ]]; then
    cmd+=(--max-trackers "$max_trackers")
  fi
  if [[ -n "$index_url_template" ]]; then
    cmd+=(--index-url-template "$index_url_template")
  fi
  if [[ -n "$index_query" ]]; then
    cmd+=(--index-query "$index_query")
  fi
  if [[ -n "$index_kind" ]]; then
    cmd+=(--index-kind "$index_kind")
  fi
  if [[ -n "$index_cache_ttl_secs" ]]; then
    cmd+=(--index-cache-ttl-secs "$index_cache_ttl_secs")
  fi
  if [[ -n "$index_timeout_secs" ]]; then
    cmd+=(--index-timeout-secs "$index_timeout_secs")
  fi

  echo "==> libtorrent soak case: $name"
  printf 'command:' > "$log_path"
  printf ' %q' "${cmd[@]}" >> "$log_path"
  printf '\n\n' >> "$log_path"

  if [[ "$DRY_RUN" == "1" ]]; then
    printf '%q ' "${cmd[@]}"
    printf '\n'
    printf "%s\tdry-run\t0\t0\t%s\t%s\n" "$name" "$log_path" "$case_dir" >> "$SUMMARY"
    return 0
  fi

  local start_ts end_ts exit_code status
  start_ts="$(date +%s)"
  set +e
  "${cmd[@]}" >> "$log_path" 2>&1
  exit_code=$?
  set -e
  end_ts="$(date +%s)"

  if [[ "$exit_code" -eq 0 ]]; then
    status="completed"
  elif [[ "$exit_code" -eq 124 && "$REQUIRE_COMPLETE" != "1" ]]; then
    status="timed-out"
  else
    status="failed"
    failed_count=$((failed_count + 1))
  fi

  printf "%s\t%s\t%s\t%s\t%s\t%s\n" \
    "$name" "$status" "$exit_code" "$((end_ts - start_ts))" "$log_path" "$case_dir" >> "$SUMMARY"

  if [[ "$status" == "failed" && "$FAIL_FAST" == "1" ]]; then
    echo "error: case '$name' failed; see $log_path" >&2
    exit 1
  fi
}

while IFS=$'\t' read -r raw_name raw_locator raw_tracker_file raw_tracker_list_url raw_discovery_file raw_discovery_url raw_timeout raw_max_trackers raw_index_url_template raw_index_query raw_index_kind raw_index_cache_ttl raw_index_timeout _extra; do
  [[ -z "${raw_name:-}" ]] && continue
  [[ "$raw_name" =~ ^[[:space:]]*# ]] && continue
  if [[ "$raw_name" == "name" && "${raw_locator:-}" == "locator" ]]; then
    continue
  fi
  if [[ -n "${_extra:-}" ]]; then
    echo "error: matrix row '$raw_name' has too many columns" >&2
    exit 1
  fi

  name="$(field_or_empty "$raw_name")"
  locator="$(field_or_empty "${raw_locator:-}")"
  tracker_file="$(field_or_empty "${raw_tracker_file:-}")"
  tracker_list_url="$(field_or_empty "${raw_tracker_list_url:-}")"
  discovery_file="$(field_or_empty "${raw_discovery_file:-}")"
  discovery_url="$(field_or_empty "${raw_discovery_url:-}")"
  timeout_secs="$(field_or_empty "${raw_timeout:-}")"
  max_trackers="$(field_or_empty "${raw_max_trackers:-}")"
  index_url_template="$(field_or_empty "${raw_index_url_template:-}")"
  index_query="$(field_or_empty "${raw_index_query:-}")"
  index_kind="$(field_or_empty "${raw_index_kind:-}")"
  index_cache_ttl_secs="$(field_or_empty "${raw_index_cache_ttl:-}")"
  index_timeout_secs="$(field_or_empty "${raw_index_timeout:-}")"
  name="${name%$'\r'}"
  locator="${locator%$'\r'}"
  tracker_file="${tracker_file%$'\r'}"
  tracker_list_url="${tracker_list_url%$'\r'}"
  discovery_file="${discovery_file%$'\r'}"
  discovery_url="${discovery_url%$'\r'}"
  timeout_secs="${timeout_secs%$'\r'}"
  max_trackers="${max_trackers%$'\r'}"
  index_url_template="${index_url_template%$'\r'}"
  index_query="${index_query%$'\r'}"
  index_kind="${index_kind%$'\r'}"
  index_cache_ttl_secs="${index_cache_ttl_secs%$'\r'}"
  index_timeout_secs="${index_timeout_secs%$'\r'}"

  if [[ -z "$name" ]]; then
    echo "error: matrix row has blank name" >&2
    exit 1
  fi
  if [[ -z "$locator" && -z "$discovery_file" && -z "$discovery_url" && -z "$index_url_template" ]]; then
    echo "error: matrix row '$name' needs locator, discovery_file, discovery_url, or index_url_template" >&2
    exit 1
  fi
  if [[ -n "$timeout_secs" ]]; then
    validate_positive_integer "timeout_secs for '$name'" "$timeout_secs"
  fi
  if [[ -n "$max_trackers" ]]; then
    validate_positive_integer "max_trackers for '$name'" "$max_trackers"
  fi
  if [[ -n "$index_cache_ttl_secs" ]]; then
    validate_positive_integer "index_cache_ttl_secs for '$name'" "$index_cache_ttl_secs"
  fi
  if [[ -n "$index_timeout_secs" ]]; then
    validate_positive_integer "index_timeout_secs for '$name'" "$index_timeout_secs"
  fi

  case_count=$((case_count + 1))
  run_case "$name" "$locator" "$tracker_file" "$tracker_list_url" "$discovery_file" "$discovery_url" "$timeout_secs" "$max_trackers" "$index_url_template" "$index_query" "$index_kind" "$index_cache_ttl_secs" "$index_timeout_secs"
done < "$MATRIX_FILE"

if [[ "$case_count" -eq 0 ]]; then
  echo "error: matrix file '$MATRIX_FILE' did not contain any runnable cases" >&2
  exit 1
fi

echo "Libtorrent public soak summary: $SUMMARY"
if [[ "$failed_count" -gt 0 ]]; then
  echo "error: $failed_count soak case(s) failed" >&2
  exit 1
fi
