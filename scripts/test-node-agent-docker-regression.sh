#!/usr/bin/env bash
set -Eeuo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_dir"

port="${CTMCP_DOCKER_REGRESSION_PORT:-3789}"
host_repo="${CTMCP_DOCKER_REGRESSION_HOST_REPO:-$repo_dir}"
raw_project="${CTMCP_DOCKER_REGRESSION_PROJECT:-ctmcp-docker-regression-${GITHUB_RUN_ID:-$$}}"
project="$(printf '%s' "$raw_project" | tr '[:upper:]_' '[:lower:]-' | sed 's/[^a-z0-9-]/-/g')"
runtime_image="${CTMCP_DOCKER_REGRESSION_RUNTIME_IMAGE:-coding-tools-node-agent:${project}}"
dev_image="${CTMCP_DOCKER_REGRESSION_DEV_IMAGE:-coding-tools-node-agent-dev:${project}}"
prod_name="${project}-prod"
scratch_rel=".local/${project}"
scratch_local="${repo_dir}/${scratch_rel}"
scratch_host="${host_repo}/${scratch_rel}"
data_local="${scratch_local}/data"
workspace_local="${scratch_local}/workspace"
data_host="${scratch_host}/data"
workspace_host="${scratch_host}/workspace"
override_file="${scratch_local}/compose.override.yml"
password="${CTMCP_DOCKER_REGRESSION_PASSWORD:-docker-regression-password}"
keep="${CTMCP_DOCKER_REGRESSION_KEEP:-0}"

export CTMCP_NODE_AGENT_IMAGE="$runtime_image"
export CTMCP_DEV_IMAGE="$dev_image"
export CTMCP_DEV_PORT="$port"
export CTMCP_OAUTH_PASSWORD="$password"
export CTMCP_WORKSPACE="$workspace_host"
export CTMCP_REGRESSION_DATA_HOST="$data_host"
export CTMCP_REGRESSION_WORKSPACE_HOST="$workspace_host"

compose=(docker compose -p "$project" -f docker-compose.dev.yml -f "$override_file")

fail() {
  printf 'docker-regression: %s\n' "$*" >&2
  return 1
}

wait_container_http() {
  local container="$1"
  local path="$2"
  local attempts="${3:-60}"
  local attempt
  for attempt in $(seq 1 "$attempts"); do
    if docker exec "$container" node -e "fetch('http://127.0.0.1:3789${path}').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

cleanup() {
  local code=$?
  set +e
  if (( code != 0 )); then
    printf '%s\n' '--- Docker regression diagnostics ---' >&2
    docker logs "$prod_name" >&2 2>/dev/null || true
    "${compose[@]}" logs --no-color dev >&2 2>/dev/null || true
    docker ps -a --filter "label=com.docker.compose.project=${project}" >&2 2>/dev/null || true
  fi
  docker rm -f "$prod_name" >/dev/null 2>&1 || true
  "${compose[@]}" down --remove-orphans >/dev/null 2>&1 || true
  if [[ "$keep" != "1" ]]; then
    rm -rf "$scratch_local"
    docker image rm -f "$dev_image" "$runtime_image" >/dev/null 2>&1 || true
  else
    printf 'docker-regression: kept artifacts in %s\n' "$scratch_local"
  fi
  return "$code"
}
trap cleanup EXIT

command -v docker >/dev/null || fail 'docker is required'
docker compose version >/dev/null

mkdir -p "$data_local" "$workspace_local"
printf '%s\n' 'docker-regression-workspace' > "${workspace_local}/regression-marker"
cat > "$override_file" <<'YAML'
services:
  dev:
    volumes:
      - "${CTMCP_REGRESSION_DATA_HOST}:/data"
      - "${CTMCP_REGRESSION_WORKSPACE_HOST}:/workspace"
      - /var/run/docker.sock:/var/run/docker.sock
YAML

printf '%s\n' '[1/8] Build runtime + dev images'
"${compose[@]}" build dev
[[ "$(docker image inspect "$dev_image" --format '{{json .Config.Cmd}}')" == "null" ]] || fail 'dev image must not carry a default CMD'

printf '%s\n' '[2/8] Start runtime container on the takeover port'
docker run -d --name "$prod_name" --user 0:0 -p "127.0.0.1:${port}:3789" -e CTMCP_OAUTH_PASSWORD="$password" "$runtime_image" >/dev/null
wait_container_http "$prod_name" /health 60 || fail 'runtime health endpoint did not become ready'

printf '%s\n' '[3/8] Stop runtime and let dev take over the same port'
docker stop "$prod_name" >/dev/null
"${compose[@]}" up -d --no-build dev >/dev/null
dev_id="$("${compose[@]}" ps -q dev)"
[[ -n "$dev_id" ]] || fail 'dev container id was not found'
wait_container_http "$dev_id" /health 60 || fail 'dev health endpoint did not become ready'
wait_container_http "$dev_id" /ui/ 10 || fail 'dev management UI did not become ready'
[[ "$(docker inspect "$dev_id" --format '{{json .Config.Cmd}}')" == "null" ]] || fail 'running dev container unexpectedly has a command override'
published_port="$(docker inspect "$dev_id" --format '{{(index (index .NetworkSettings.Ports "3789/tcp") 0).HostPort}}')"
[[ "$published_port" == "$port" ]] || fail "expected published port ${port}, got ${published_port}"

printf '%s\n' '[4/8] Verify Node Agent process contract and privilege drop'
process_table="$(docker exec "$dev_id" ps -eo user=,pid=,args=)"
printf '%s\n' "$process_table" | grep -Eq '^node[[:space:]]+[0-9]+[[:space:]]+node dist/cli\.js --restart-supervised[[:space:]]*$' || fail 'Node Agent is not running as node with the expected arguments'
if printf '%s\n' "$process_table" | grep -Fq -- '--restart-supervised bash'; then
  fail 'stray bash argument reached the Node Agent'
fi

printf '%s\n' '[5/8] Verify Docker socket and development toolchain as node'
docker exec -u node "$dev_id" docker version --format '{{.Server.Version}}' >/dev/null || fail 'node user cannot access the Docker socket'
docker exec -u node "$dev_id" bash -lc 'cargo --version >/dev/null && rustc --version >/dev/null' || fail 'Rust/Cargo toolchain is unavailable to node'

printf '%s\n' '[6/8] Verify workspace and isolated data mounts'
docker exec -u node "$dev_id" test -f /workspace/regression-marker || fail 'workspace mount is missing'
docker exec -u node "$dev_id" test -w /data || fail 'data directory is not writable by node'
actual_data="$(docker inspect "$dev_id" --format '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Source}}{{end}}{{end}}')"
actual_workspace="$(docker inspect "$dev_id" --format '{{range .Mounts}}{{if eq .Destination "/workspace"}}{{.Source}}{{end}}{{end}}')"
[[ "$actual_data" == "$data_host" ]] || fail "unexpected /data source: ${actual_data}"
[[ "$actual_workspace" == "$workspace_host" ]] || fail "unexpected /workspace source: ${actual_workspace}"
[[ "$actual_data" != "${host_repo}/.data" ]] || fail 'dev data must not share the production .data directory'

printf '%s\n' '[7/8] Restart dev and verify recovery'
docker restart "$dev_id" >/dev/null
wait_container_http "$dev_id" /health 60 || fail 'dev did not recover after restart'
wait_container_http "$dev_id" /ui/ 10 || fail 'dev UI did not recover after restart'
docker exec -u node "$dev_id" docker version --format '{{.Server.Version}}' >/dev/null || fail 'Docker socket access was lost after restart'

printf '%s\n' '[8/8] Scan startup logs for deployment regressions'
logs="$(docker logs "$dev_id" 2>&1)"
if printf '%s\n' "$logs" | grep -Eqi 'EADDRINUSE|permission denied|(^|[^[:alpha:]])fatal([^[:alpha:]]|$)'; then
  printf '%s\n' "$logs" >&2
  fail 'startup logs contain a deployment regression signal'
fi

printf 'docker-regression=ok port=%s project=%s\n' "$port" "$project"
