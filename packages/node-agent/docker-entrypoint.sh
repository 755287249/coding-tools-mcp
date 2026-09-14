#!/bin/sh
set -eu

config_file="${CTMCP_CONFIG_FILE:-${CTMCP_DATA_DIR:-/data}/agent.json}"

if [ "$(id -u)" = "0" ]; then
  mkdir -p "${CTMCP_DATA_DIR:-/data}" /workspace
  chown -R node:node "${CTMCP_DATA_DIR:-/data}"

  if [ -S /var/run/docker.sock ]; then
    docker_gid="$(stat -c '%g' /var/run/docker.sock)"
    docker_group="$(getent group "$docker_gid" | cut -d: -f1 || true)"
    if [ -z "$docker_group" ]; then
      docker_group="docker-host"
      groupadd --gid "$docker_gid" "$docker_group"
    fi
    usermod -aG "$docker_group" node
  fi

  exec gosu node "$0" "$@"
fi

if [ ! -f "$config_file" ]; then
  mkdir -p "$(dirname "$config_file")"
  cat > "$config_file" <<'EOF'
{
  "schema_version": 1,
  "host": "0.0.0.0",
  "port": 3789,
  "dataDir": "/data",
  "folders": [
    {
      "path": "/workspace",
      "name": "workspace"
    }
  ],
  "management": {
    "enabled": true
  },
  "sandbox": {
    "enabled": false
  },
  "tunnel": {
    "enabled": false
  }
}
EOF
fi

export CTMCP_RESTART_SUPERVISOR=active-v1
exec node dist/cli.js --restart-supervised "$@"
