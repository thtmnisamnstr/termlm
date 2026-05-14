#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

LEVEL="${TERMLM_ZSH_USABILITY_LEVEL:-smoke}"
PROFILE="${TERMLM_ZSH_USABILITY_PROFILE:-debug}"
SKIP_BUILD="${TERMLM_ZSH_USABILITY_SKIP_BUILD:-0}"
REQUIRE_MODEL="${TERMLM_ZSH_USABILITY_REQUIRE_MODEL:-0}"
EXPECT_TIMEOUT="${TERMLM_ZSH_USABILITY_TIMEOUT_SECS:-240}"
MODEL_DIR="${TERMLM_ZSH_USABILITY_MODEL_DIR:-${HOME}/.local/share/termlm/models}"
KEEP_ARTIFACTS="${TERMLM_ZSH_USABILITY_KEEP_ARTIFACTS:-0}"

case "${LEVEL}" in
  smoke|release|full) ;;
  *)
    echo "zsh usability: unknown TERMLM_ZSH_USABILITY_LEVEL='${LEVEL}'" >&2
    exit 2
    ;;
esac

case "${PROFILE}" in
  debug)
    BUILD_DIR="${ROOT_DIR}/target/debug"
    BUILD_CMD=(cargo build -p termlm-client -p termlm-core)
    ;;
  release)
    BUILD_DIR="${ROOT_DIR}/target/release"
    BUILD_CMD=(cargo build -p termlm-client -p termlm-core --release --locked)
    ;;
  *)
    echo "zsh usability: unknown TERMLM_ZSH_USABILITY_PROFILE='${PROFILE}'" >&2
    exit 2
    ;;
esac

CLIENT_BIN="${TERMLM_ZSH_USABILITY_CLIENT_BIN:-${BUILD_DIR}/termlm-client}"
CORE_BIN="${TERMLM_ZSH_USABILITY_CORE_BIN:-${BUILD_DIR}/termlm-core}"
if [[ ! -x "${CLIENT_BIN}" && -x "${BUILD_DIR}/termlm" ]]; then
  CLIENT_BIN="${BUILD_DIR}/termlm"
fi

E4B_MODEL="${MODEL_DIR}/gemma-4-E4B-it-Q4_K_M.gguf"
EMBED_MODEL="${MODEL_DIR}/bge-small-en-v1.5.Q4_K_M.gguf"
if [[ ! -f "${E4B_MODEL}" || ! -f "${EMBED_MODEL}" ]]; then
  msg="zsh usability: missing local model assets in ${MODEL_DIR}; need $(basename "${E4B_MODEL}") and $(basename "${EMBED_MODEL}")"
  if [[ "${REQUIRE_MODEL}" == "1" ]]; then
    echo "${msg}" >&2
    exit 1
  fi
  echo "${msg}; skipping real zsh usability suite"
  exit 0
fi

if ! command -v expect >/dev/null 2>&1; then
  echo "zsh usability: expect is required" >&2
  exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "zsh usability: python3 is required" >&2
  exit 1
fi

if [[ "${SKIP_BUILD}" != "1" ]]; then
  (cd "${ROOT_DIR}" && "${BUILD_CMD[@]}")
fi
[[ -x "${CLIENT_BIN}" ]] || { echo "zsh usability: missing client binary ${CLIENT_BIN}" >&2; exit 1; }
[[ -x "${CORE_BIN}" ]] || { echo "zsh usability: missing core binary ${CORE_BIN}" >&2; exit 1; }

if [[ -n "${TERMLM_ZSH_USABILITY_ARTIFACT_DIR:-}" ]]; then
  TMP_ROOT="${TERMLM_ZSH_USABILITY_ARTIFACT_DIR}"
  KEEP_ARTIFACTS=1
  rm -rf -- "${TMP_ROOT}"
  mkdir -p -- "${TMP_ROOT}"
else
  TMP_PARENT="${TMPDIR:-/tmp}"
  TMP_PARENT="${TMP_PARENT%/}"
  TMP_ROOT="$(mktemp -d "${TMP_PARENT}/termlm-zsh-usability.XXXXXX")"
fi

SERVER_PID=""
cleanup() {
  local status=$?
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -x "${CLIENT_BIN}" && -d "${HOME_DIR:-/nonexistent}" ]]; then
    HOME="${HOME_DIR}" \
      XDG_RUNTIME_DIR="${RUNTIME_DIR:-${TMP_ROOT}/runtime}" \
      "${CLIENT_BIN}" stop >/dev/null 2>&1 || true
  fi
  if [[ "${KEEP_ARTIFACTS}" != "1" ]]; then
    rm -rf -- "${TMP_ROOT}"
  else
    echo "zsh usability artifacts: ${TMP_ROOT}"
  fi
  exit "${status}"
}
trap cleanup EXIT

HOME_DIR="${TMP_ROOT}/home"
RUNTIME_DIR="${TMP_ROOT}/runtime"
STATE_DIR="${TMP_ROOT}/state"
DATA_DIR="${TMP_ROOT}/data"
CONFIG_DIR="${HOME_DIR}/.config/termlm"
ZDOTDIR_PATH="${TMP_ROOT}/zdotdir"
WORK_DIR="${TMP_ROOT}/workspace"
BIN_DIR="${TMP_ROOT}/path-bin"
TRACE_DIR="${TMP_ROOT}/retrieval-traces"
EXPECT_LOG="${TMP_ROOT}/zsh-transcript.log"
WEB_LOG="${TMP_ROOT}/web.log"
DAEMON_LOG="${STATE_DIR}/termlm/termlm.log"
SERVER_SCRIPT="${TMP_ROOT}/web_server.py"
mkdir -p \
  "${HOME_DIR}/Desktop/nested/images" \
  "${HOME_DIR}/Desktop/notes/deeper" \
  "${HOME_DIR}/Downloads" \
  "${HOME_DIR}/Documents" \
  "${RUNTIME_DIR}" \
  "${STATE_DIR}/termlm" \
  "${DATA_DIR}" \
  "${CONFIG_DIR}" \
  "${ZDOTDIR_PATH}" \
  "${BIN_DIR}" \
  "${WORK_DIR}/src" \
  "${WORK_DIR}/docs" \
  "${WORK_DIR}/packages/a" \
  "${TRACE_DIR}"

cat > "${WORK_DIR}/package.json" <<'JSON'
{"scripts":{"test":"vitest run","build":"vite build"},"devDependencies":{"vitest":"latest"}}
JSON
printf 'alpha\n' > "${WORK_DIR}/alpha.txt"
printf '# Bravo\n' > "${WORK_DIR}/bravo.md"
printf 'hidden\n' > "${WORK_DIR}/.env"
printf 'visible\n' > "${WORK_DIR}/visible.txt"
printf 'TODO uppercase\n' > "${WORK_DIR}/src/app.py"
printf 'todo lowercase\n' > "${WORK_DIR}/docs/todo.txt"
touch "${WORK_DIR}/README.md" "${WORK_DIR}/docs/README.md" "${WORK_DIR}/packages/a/README.md"
dd if=/dev/zero of="${WORK_DIR}/large-a.bin" bs=1024 count=20 status=none
dd if=/dev/zero of="${WORK_DIR}/large-b.bin" bs=1024 count=12 status=none
dd if=/dev/zero of="${WORK_DIR}/large-c.bin" bs=1024 count=6 status=none
mkdir -p "${WORK_DIR}/empty-delete-a" "${WORK_DIR}/empty-delete-b" "${WORK_DIR}/nonempty-delete"
printf 'keep\n' > "${WORK_DIR}/nonempty-delete/keep.txt"
for i in 1 2 3 4 5 6 7 8; do
  printf 'line%s\n' "${i}" >> "${WORK_DIR}/app.log"
done
printf 'old\n' > "${WORK_DIR}/oldest.txt"
printf 'new\n' > "${WORK_DIR}/newest.txt"
touch -t 202401010101 "${WORK_DIR}/oldest.txt"
touch -t 202501010101 "${WORK_DIR}/newest.txt"
printf 'download 1\n' > "${HOME_DIR}/Downloads/one.txt"
printf 'download 2\n' > "${HOME_DIR}/Downloads/two.pdf"
printf 'download 3\n' > "${HOME_DIR}/Downloads/three.md"
mkdir -p "${HOME_DIR}/Downloads/archive"
printf 'document content\n' > "${HOME_DIR}/Documents/report.md"
dd if=/dev/zero of="${HOME_DIR}/Documents/data.bin" bs=1024 count=4 status=none
printf '# root markdown\n' > "${HOME_DIR}/Desktop/root.md"
printf '# nested markdown\n' > "${HOME_DIR}/Desktop/notes/deeper/nested.markdown"
printf 'not markdown\n' > "${HOME_DIR}/Desktop/notes/readme.txt"
printf 'jpg\n' > "${HOME_DIR}/Desktop/photo.jpg"
printf 'png\n' > "${HOME_DIR}/Desktop/nested/images/nested.png"
printf 'gif\n' > "${HOME_DIR}/Desktop/nested/images/anim.gif"

for cmd in pwd ls find grep tail head wc du mkdir date cp xargs stat sort cut printf test echo cat sed awk; do
  src="$(command -v "${cmd}" 2>/dev/null || true)"
  if [[ "${src}" == /* ]]; then
    ln -sf -- "${src}" "${BIN_DIR}/${cmd}"
  fi
done
if src="$(command -v rg 2>/dev/null)"; then
  ln -sf -- "${src}" "${BIN_DIR}/rg"
fi
cat > "${BIN_DIR}/code" <<'EOF'
#!/usr/bin/env bash
printf 'code fixture invoked: %s\n' "$*"
EOF
chmod +x "${BIN_DIR}/code"

pick_free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

WEB_PORT="$(pick_free_port)"
cat > "${SERVER_SCRIPT}" <<'PY'
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

port = int(sys.argv[1])
log_path = sys.argv[2]

release_body = """
WidgetCLI 9.4.2 release notes

The current stable WidgetCLI release is 9.4.2. The recommended upgrade command for
interactive terminal users is:

    widgetctl upgrade --fast --channel stable-2026q2

Use this command only after reviewing your current project lockfile. These notes are
served by the termlm local usability test web fixture. The surrounding paragraph is
intentionally verbose so the page has enough readable text for extraction. WidgetCLI
9.4.2 improves command planning, shell safety, release notes lookup, and documentation
indexing. The exact recommended upgrade command remains widgetctl upgrade --fast
--channel stable-2026q2.
"""

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def record(self):
        with open(log_path, "a", encoding="utf-8") as f:
            f.write(self.path + "\n")

    def do_GET(self):
        self.record()
        parsed = urlparse(self.path)
        if parsed.path == "/search":
            body = {
                "results": [
                    {
                        "url": f"http://127.0.0.1:{port}/widgetcli-release",
                        "title": "WidgetCLI 9.4.2 Release Notes",
                        "snippet": "Release notes for WidgetCLI 9.4.2. Read the page for the recommended upgrade command.",
                    }
                ]
            }
            data = json.dumps(body).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        if parsed.path == "/widgetcli-release":
            data = release_body.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        self.send_response(404)
        self.end_headers()

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
python3 "${SERVER_SCRIPT}" "${WEB_PORT}" "${WEB_LOG}" &
SERVER_PID=$!
sleep 0.3
kill -0 "${SERVER_PID}" >/dev/null 2>&1 || {
  echo "zsh usability: local web fixture failed to start" >&2
  exit 1
}

json_string() {
  python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"
}

cat > "${CONFIG_DIR}/config.toml" <<EOF
[inference]
provider = "local"
tool_calling_required = true
stream = true
token_idle_timeout_secs = 45
startup_failure_behavior = "fail"

[performance]
profile = "performance"
warm_core_on_start = true
keep_embedding_warm = true
prewarm_common_docs = true

[model]
auto_download = false
models_dir = $(json_string "${MODEL_DIR}")
e4b_filename = "gemma-4-E4B-it-Q4_K_M.gguf"
context_tokens = 8192

[daemon]
socket_path = "\$XDG_RUNTIME_DIR/termlm.sock"
pid_file = "\$XDG_RUNTIME_DIR/termlm.pid"
log_file = $(json_string "${DAEMON_LOG}")
shutdown_grace_secs = 1
boot_timeout_secs = ${EXPECT_TIMEOUT}

[web]
enabled = true
expose_tools = true
provider = "custom_json"
search_endpoint = "http://127.0.0.1:${WEB_PORT}/search"
request_timeout_secs = 5
connect_timeout_secs = 2
max_results = 3
max_pages_per_task = 2
allowed_schemes = ["http", "https"]
allow_plain_http = true
allow_local_addresses = true
obey_robots_txt = false
citation_required = false
min_delay_between_requests_ms = 0
search_cache_ttl_secs = 0

[web.extract]
min_extracted_chars = 80
max_markdown_bytes = 12000

[indexer]
enabled = true
max_loadavg = 128.0
concurrency = 4
max_binaries = 120
max_doc_bytes = 65536
embedding_provider = "local"
query_embedding_timeout_secs = 4
embed_filename = "bge-small-en-v1.5.Q4_K_M.gguf"
hybrid_retrieval_enabled = true
lexical_index_enabled = true
command_aware_retrieval = true
rag_top_k = 8
lexical_top_k = 50

[behavior]
thinking = false
allow_clarifications = true
max_tool_rounds = 10
max_planning_rounds = 5
command_timeout_secs = 120

[terminal_context]
enabled = true
capture_all_interactive_commands = true
capture_command_output = true
max_entries = 100

[local_tools]
enabled = true
readonly_command_enabled = true
readonly_command_timeout_secs = 5
readonly_command_max_output_bytes = 65536
allow_home_as_workspace = true
sensitive_path_allowlist = [$(json_string "${HOME_DIR}")]
max_workspace_entries = 1000
max_search_results = 200

[debug]
retrieval_trace_enabled = true
retrieval_trace_dir = $(json_string "${TRACE_DIR}")
retrieval_trace_max_files = 100

[prompt]
indicator = "TERMLM_PROMPT> "
session_indicator = "TERMLM_SESSION> "
use_color = false
EOF

cat > "${ZDOTDIR_PATH}/.zshrc" <<EOF
export HOME=$(json_string "${HOME_DIR}")
export XDG_RUNTIME_DIR=$(json_string "${RUNTIME_DIR}")
export XDG_CONFIG_HOME=$(json_string "${HOME_DIR}/.config")
export XDG_DATA_HOME=$(json_string "${DATA_DIR}")
export XDG_STATE_HOME=$(json_string "${STATE_DIR}")
export TERMLM_DATA_DIR=$(json_string "${DATA_DIR}")
export TERMLM_FILESYSTEM_CONTEXT_PATH=$(json_string "${DATA_DIR}/context/filesystem.md")
export TERMLM_CLIENT_BIN=$(json_string "${CLIENT_BIN}")
export TERMLM_CORE_BIN=$(json_string "${CORE_BIN}")
export TERMLM_DAEMON_BOOT_TIMEOUT_SECS="${EXPECT_TIMEOUT}"
export TERMLM_PROMPT_USE_COLOR=0
export PATH=$(json_string "${BIN_DIR}:/bin:/usr/bin:/usr/sbin:/sbin:/opt/homebrew/bin")
export PS1='TERMLM_NORMAL> '
bindkey -e
cd $(json_string "${WORK_DIR}")
source $(json_string "${ROOT_DIR}/plugins/zsh/termlm.plugin.zsh")
EOF

export TERMLM_EXPECT_ZDOTDIR="${ZDOTDIR_PATH}"
export TERMLM_EXPECT_TIMEOUT="${EXPECT_TIMEOUT}"
export TERMLM_EXPECT_LEVEL="${LEVEL}"
export TERMLM_EXPECT_WORK_DIR="${WORK_DIR}"
export TERMLM_EXPECT_HOME_DIR="${HOME_DIR}"

if ! expect <<'EOF' >"${EXPECT_LOG}" 2>&1
set timeout $env(TERMLM_EXPECT_TIMEOUT)
set level $env(TERMLM_EXPECT_LEVEL)
set work_dir $env(TERMLM_EXPECT_WORK_DIR)
set home_dir $env(TERMLM_EXPECT_HOME_DIR)
set prompt {TERMLM_NORMAL>}
set bad_response {I couldn't produce|couldn't produce|What exact command behavior|blank response|No response|provider stream error|provider request failed|local inference failed|Decode Error|\[multimodal\]}

proc fail {name msg} {
  puts stderr "zsh usability failure in $name: $msg"
  exit 1
}

proc wait_normal {name} {
  global prompt
  expect {
    -re $prompt {}
    eof { fail $name "shell exited while waiting for normal prompt" }
    timeout { fail $name "timed out waiting for normal prompt" }
  }
}

proc enter_prompt {name text} {
  send -- "?"
  expect {
    -re {TERMLM_PROMPT>} {}
    eof { fail $name "shell exited before prompt mode" }
    timeout { fail $name "timed out entering prompt mode" }
  }
  send -- "$text\r"
}

proc expect_no_blank_failure {name} {
  global bad_response
  expect {
    -re $bad_response {
      fail $name "model produced a blank/fallback failure"
    }
    timeout { return }
    -re {TERMLM_NORMAL>} { return }
  }
}

proc run_answer {name text answer_re} {
  global bad_response
  enter_prompt $name $text
  expect {
    -re $answer_re {}
    -re {proposed command} { fail $name "expected a direct answer, got a command proposal" }
    -re $bad_response { fail $name "unhelpful fallback response" }
    eof { fail $name "shell exited" }
    timeout { fail $name "timed out waiting for answer /$answer_re/" }
  }
  wait_normal $name
  puts "PASS $name"
}

proc run_answer_or_command {name text answer_re} {
  global bad_response
  enter_prompt $name $text
  expect {
    -re {proposed command} {
      expect -re {keys}
      send -- "y"
      expect {
        -re $answer_re {}
        eof { fail $name "shell exited before accepted command output" }
        timeout { fail $name "timed out waiting for accepted command output /$answer_re/" }
      }
      wait_normal $name
    }
    -re $answer_re {
      wait_normal $name
    }
    -re $bad_response { fail $name "unhelpful fallback response" }
    eof { fail $name "shell exited" }
    timeout { fail $name "timed out waiting for answer or command" }
  }
  puts "PASS $name"
}

proc run_command {name text output_re} {
  global bad_response
  enter_prompt $name $text
  expect {
    -re {proposed command} {}
    -re $bad_response { fail $name "unhelpful fallback response" }
    eof { fail $name "shell exited" }
    timeout { fail $name "timed out waiting for command proposal" }
  }
  expect -re {keys}
  send -- "y"
  if {$output_re ne ""} {
    expect {
      -re $output_re {}
      eof { fail $name "shell exited before command output" }
      timeout { fail $name "timed out waiting for command output /$output_re/" }
    }
  }
  wait_normal $name
  puts "PASS $name"
}

proc run_command_then_marker {name text marker_cmd marker_re} {
  run_command $name $text ""
  send -- "$marker_cmd\r"
  expect {
    -re $marker_re {}
    eof { fail $name "shell exited before marker check" }
    timeout { fail $name "marker check failed /$marker_re/" }
  }
  wait_normal $name
}

proc run_rejecting_command {name text first_re second_re} {
  global bad_response
  enter_prompt $name $text
  expect {
    -re {proposed command} {}
    -re $bad_response { fail $name "unhelpful fallback response" }
    eof { fail $name "shell exited" }
    timeout { fail $name "timed out waiting for command proposal" }
  }
  if {$first_re ne ""} {
    expect {
      -re $first_re {}
      timeout { fail $name "proposal missing /$first_re/" }
    }
  }
  if {$second_re ne ""} {
    expect {
      -re $second_re {}
      timeout { fail $name "proposal missing /$second_re/" }
    }
  }
  send -- "n"
  wait_normal $name
  puts "PASS $name"
}

proc run_answer_or_rejecting_command {name text answer_re command_re} {
  global bad_response
  enter_prompt $name $text
  expect {
    -re $answer_re {
      wait_normal $name
    }
    -re {proposed command} {
      expect {
        -re $command_re {}
        timeout { fail $name "proposal missing /$command_re/" }
      }
      send -- "n"
      wait_normal $name
    }
    -re $bad_response { fail $name "unhelpful fallback response" }
    eof { fail $name "shell exited" }
    timeout { fail $name "timed out waiting for answer or proposal" }
  }
  puts "PASS $name"
}

spawn env TERM=xterm-256color ZDOTDIR=$env(TERMLM_EXPECT_ZDOTDIR) zsh -i
wait_normal boot

send -- "echo __normal_before_ok__\r"
expect -re {__normal_before_ok__}
wait_normal normal-before

run_answer direct-pwd "What directory am I in?" [string map [list "/" "\\/"] $work_dir]

send -- "printf terminal-context-token > context-note.txt\r"
wait_normal context-command
run_answer_or_command terminal-context "what command did I just run?" {context-note|terminal-context-token|printf}

run_answer_or_command downloads-count "how many files do I have in my downloads?" {(^|[^0-9])3([^0-9]|$)|three}
run_answer_or_command project-test-script "what npm command runs tests in this project?" {vitest run|npm test|npm run test}

run_command files-not-directories "list all files but not directories in this directory" {alpha\.txt|bravo\.md}
run_command directories-only "show only directories, not files" {src|docs|packages}
run_command largest-files "show me the 3 largest files here, then I'll decide what to delete" {large-a\.bin}
run_command hidden-files "show hidden files in this folder" {\.env}
run_command find-readmes "find every README.md under here" {docs/README\.md|packages/a/README\.md}
run_command tail-log "show the last 5 lines of app.log" {line4}
run_command search-todo "search recursively for TODO case insensitively in this directory" {TODO uppercase|todo lowercase}
run_answer_or_command oldest-file "what is the oldest file in this directory?" {oldest\.txt}
run_answer_or_command documents-usage "how much storage is being used by my Documents folder?" {Documents|[0-9]+[KMG]?}

run_command empty-directory-delete "find every empty directory under here and delete them" ""
send -- "test ! -e empty-delete-a -a ! -e empty-delete-b -a -e nonempty-delete/keep.txt && echo __empty_delete_ok__ || echo __empty_delete_fail__\r"
expect {
  -re {__empty_delete_ok__} {}
  -re {__empty_delete_fail__} { fail empty-directory-delete "empty directories were not removed correctly" }
  timeout { fail empty-directory-delete "timed out checking empty directory deletion" }
}
wait_normal empty-directory-delete-check

run_command markdown-copy "make a folder on my desktop named \"md\" and copy all markdown files from my desktop and all of its subfolders recursive to the new \"md\" folder" ""
send -- "for i in {1..20}; do test -f \"$home_dir/Desktop/md/root.md\" -a -f \"$home_dir/Desktop/md/nested.markdown\" && echo __md_copy_ok__ && break; sleep 0.25; done; test -f \"$home_dir/Desktop/md/root.md\" -a -f \"$home_dir/Desktop/md/nested.markdown\" || echo __md_copy_fail__\r"
expect {
  -re {__md_copy_ok__} {}
  -re {__md_copy_fail__} { fail markdown-copy "copied Markdown files were not present" }
  timeout { fail markdown-copy "timed out checking copied Markdown files" }
}
wait_normal markdown-copy-check

run_command image-list "list all of the image files on my desktop and in all of my desktop subfolders" {photo\.jpg|nested\.png|anim\.gif}
run_command image-copy "create a new folder on my desktop named images-copy, and copy all image files from my desktop and all of its subdirectories recursively to this new folder" ""
send -- "for i in {1..20}; do test -f \"$home_dir/Desktop/images-copy/photo.jpg\" -a -f \"$home_dir/Desktop/images-copy/nested.png\" && echo __image_copy_ok__ && break; sleep 0.25; done; test -f \"$home_dir/Desktop/images-copy/photo.jpg\" -a -f \"$home_dir/Desktop/images-copy/nested.png\" || echo __image_copy_fail__\r"
expect {
  -re {__image_copy_ok__} {}
  -re {__image_copy_fail__} { fail image-copy "copied image files were not present" }
  timeout { fail image-copy "timed out checking copied image files" }
}
wait_normal image-copy-check

run_rejecting_command vscode-newest-md "list the markdown files on my desktop and open the most recent one in VS Code" {find|ls} {code}

if {$level eq "release" || $level eq "full"} {
  run_answer_or_rejecting_command web-release-notes "look up the latest WidgetCLI release notes on the web, read the result page, and tell me the exact recommended upgrade command" {widgetctl upgrade --fast --channel stable-2026q2} {widgetctl upgrade --fast --channel stable-2026q2}
}

send -- "echo __normal_after_ok__\r"
expect -re {__normal_after_ok__}
wait_normal normal-after

send -- "exit\r"
expect eof
EOF
then
  echo "zsh usability: PTY run failed; transcript follows" >&2
  sed -n '1,260p' "${EXPECT_LOG}" >&2 || true
  exit 1
fi

if [[ "${LEVEL}" == "release" || "${LEVEL}" == "full" ]]; then
  if ! grep -q '/search' "${WEB_LOG}"; then
    echo "zsh usability: expected web_search fixture hit was not observed" >&2
    exit 1
  fi
  if ! grep -q '/widgetcli-release' "${WEB_LOG}"; then
    echo "zsh usability: expected web_read fixture hit was not observed" >&2
    exit 1
  fi
fi

python3 - "${EXPECT_LOG}" <<'PY'
import re
import sys

transcript = open(sys.argv[1], errors="replace").read().replace("\r", "\n")
transcript = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", transcript)
count = transcript.count("What directory am I in?")
if count > 12:
    print(
        f"zsh usability: first prompt was redrawn too many times ({count})",
        file=sys.stderr,
    )
    sys.exit(1)
PY

trace_count="$(find "${TRACE_DIR}" -type f 2>/dev/null | wc -l | tr -d '[:space:]')"
if [[ "${trace_count}" -lt 1 ]]; then
  echo "zsh usability: expected at least one retrieval trace in ${TRACE_DIR}" >&2
  exit 1
fi

if grep -E "I couldn't produce|couldn't produce|What exact command behavior|No response|provider stream error|provider request failed|local inference failed|Decode Error|\\[multimodal\\]" "${EXPECT_LOG}" >/dev/null; then
  echo "zsh usability: transcript contains an unhelpful fallback response" >&2
  exit 1
fi

echo "zsh usability ${LEVEL} checks passed (${trace_count} retrieval traces)"
