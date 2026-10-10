# Coding Tools updates on a shared Worker

`workers/ctmcp-updates.mjs` handles only `/ctmcp/*`. Existing application routes such as `/latest`, `/ticket`, `/beta/latest`, `/dl`, `/us` and `/admin` keep their existing handler and configuration.

## Integration

For a bundled module Worker, import `ctmcpUpdates` and call it at the beginning of the existing `fetch` handler:

```js
const result = await ctmcpUpdates(request, env);
if (result) return result;
// Existing application routing follows.
```

For a single-file dashboard Worker, paste the helper without its `export` keyword above the existing export and add that dispatch call. The user's local `../docs/worker.js` has this integration; it is not copied into the repository because it contains unrelated deployment configuration.

## Runtime configuration

- `CTMCP_CHANNEL`: `stable` by default. Set `beta` for this repository’s existing personal-release workflow, which publishes prereleases. Beta selects the greatest version among the newest 100 published releases carrying the exact Coding Tools EXE name; drafts and other products are excluded.
- `CTMCP_REPO`: `755287249/coding-tools-mcp` (independent of legacy `REPO`).
- `CTMCP_GH_TOKEN`: optional GitHub release read secret. When omitted, the existing `GH_TOKEN` can be reused if it has access to both repositories. Public repositories need no token for CTMCP routes.
- Keep all existing legacy variables, secrets and KV bindings intact.
- CTMCP endpoints are public update downloads, with no legacy password/quota requirement. They only expose the selected CTMCP release asset, never arbitrary repository files.

Publish a stable GitHub Release with tag `vMAJOR.MINOR.PATCH` or `client-vMAJOR.MINOR.PATCH`, asset `ctmcp-MAJOR.MINOR.PATCH-win64.exe` and GitHub-provided `sha256:` asset digest. Drafts, setup packages and other products are rejected; prereleases require the explicit beta channel. The existing personal-release publishing workflow remains the release path; use its documented personal authorization. An absent stable release or digest fails with a clear service error.

## Deployment status

Configured client default: `https://arena.755.cc.cd/ctmcp`. Publish the updated local Worker and configure `CTMCP_REPO` before using the new route.

## Client

Open the lower-left About/info button, set the update source to the Worker HTTPS origin or its `/ctmcp` URL, and check for updates. The source is saved locally after a successful check. Automatic startup checks use that saved source and show a yellow dot for newer versions. The version window downloads and verifies the EXE; restart/install requires the user's confirmation and uses the existing visible Windows update console and rollback copy. Installation is available only in the native Windows x64 client.

- `GET /ctmcp/latest`: application identity, stable version, notes and matching portable file name/size/SHA-256.
- `GET /ctmcp/download?version=...&sha256=...`: fetches that exact current release; rejects version or digest drift with HTTP 409.

The client also accepts `owner/repository` as its existing direct GitHub update source. It validates application identity for Worker metadata, checks the expected version/filename/hash, enforces the download limit and checks the downloaded PE header and SHA-256 again before installing.

## Verification

`node --test tests/ctmcp-update-worker.test.mjs` checks independent routing, source selection, download pinning, wrong application assets, digest/size errors and provider failures. Rust `updates::` tests cover source validation and release binding. Live deployment and real release availability must be checked against the actual configured domain; passing fixtures does not establish deployment readiness.
