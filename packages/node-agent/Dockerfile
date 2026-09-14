# syntax=docker/dockerfile:1.7

FROM node:22-bookworm-slim AS build

ENV PNPM_HOME=/pnpm
ENV PATH=${PNPM_HOME}:${PATH}

WORKDIR /src

RUN npm install --global @pnpm/exe@12.0.0

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY packages/node-agent/package.json packages/node-agent/package.json

RUN pnpm install --frozen-lockfile

COPY svelte.config.js vite.config.js tsconfig.json ./
COPY src ./src
COPY static ./static
COPY scripts/extract-sveltekit-inline.mjs ./scripts/extract-sveltekit-inline.mjs
COPY packages/node-agent ./packages/node-agent

ARG BUILD_GIT_SHA=unknown
ARG BUILD_SOURCE_CLEAN=false
ARG TOOL_EVOLUTION_PROPOSAL_IDS=
ENV CTMCP_BUILD_GIT_SHA=${BUILD_GIT_SHA}
ENV CTMCP_BUILD_SOURCE_CLEAN=${BUILD_SOURCE_CLEAN}
ENV CTMCP_TOOL_EVOLUTION_PROPOSAL_IDS=${TOOL_EVOLUTION_PROPOSAL_IDS}

RUN pnpm --filter @coding-tools/node-agent run build \
    && pnpm --filter @coding-tools/node-agent --prod deploy /opt/node-agent

FROM node:22-bookworm-slim AS runtime

RUN npm install --global @pnpm/exe@12.0.0 \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git gosu openssh-client passwd \
    && install -m 0755 -d /etc/apt/keyrings \
    && curl -fsSL https://download.docker.com/linux/debian/gpg -o /etc/apt/keyrings/docker.asc \
    && chmod a+r /etc/apt/keyrings/docker.asc \
    && . /etc/os-release \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/debian ${VERSION_CODENAME} stable" > /etc/apt/sources.list.d/docker.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends docker-buildx-plugin docker-ce-cli docker-compose-plugin \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=build --chown=node:node /opt/node-agent ./
COPY --chmod=755 packages/node-agent/docker-entrypoint.sh /usr/local/bin/node-agent-entrypoint

RUN mkdir -p /data /workspace \
    && chown -R node:node /data /workspace

ENV NODE_ENV=production \
    CTMCP_DATA_DIR=/data \
    CTMCP_HOST=0.0.0.0 \
    CTMCP_PORT=3789 \
    CTMCP_WORKSPACES=/workspace \
    CTMCP_SANDBOX_ENABLED=false \
    CTMCP_SHARED_STORE_DISABLED=1 \
    DOCKER_HOST=unix:///var/run/docker.sock

VOLUME ["/data", "/workspace"]
EXPOSE 3789

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD node -e "fetch('http://127.0.0.1:' + process.env.CTMCP_PORT + '/health').then(r => { if (!r.ok) process.exit(1) }).catch(() => process.exit(1))"

ENTRYPOINT ["node-agent-entrypoint"]
