import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  toolEvolutionProposalIdsFromCommitMessage,
  toolEvolutionProposalIdsFromEnvironment
} from './build-info-lib.mjs';

const packageRoot = path.dirname(fileURLToPath(new URL('../package.json', import.meta.url)));
const repositoryRoot = path.resolve(packageRoot, '../..');
const environmentSha = String(process.env.CTMCP_BUILD_GIT_SHA ?? '').trim().toLowerCase();
const environmentClean = String(process.env.CTMCP_BUILD_SOURCE_CLEAN ?? '').trim().toLowerCase();
const hasEnvironmentProposalIds = Object.prototype.hasOwnProperty.call(process.env, 'CTMCP_TOOL_EVOLUTION_PROPOSAL_IDS');
const environmentProposalIds = process.env.CTMCP_TOOL_EVOLUTION_PROPOSAL_IDS;

let buildGitSha = /^[0-9a-f]{40}$/.test(environmentSha) ? environmentSha : 'unknown';
let sourceClean = environmentClean === 'true' || environmentClean === '1'
  ? true
  : environmentClean === 'false' || environmentClean === '0'
    ? false
    : null;
let toolEvolutionProposalIds = hasEnvironmentProposalIds
  ? toolEvolutionProposalIdsFromEnvironment(environmentProposalIds)
  : [];
try {
  if (buildGitSha === 'unknown') {
    const resolved = execFileSync('git', ['-C', repositoryRoot, 'rev-parse', 'HEAD'], {
      encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'ignore']
    }).trim().toLowerCase();
    if (/^[0-9a-f]{40}$/.test(resolved)) buildGitSha = resolved;
  }
  if (sourceClean === null) {
    const status = execFileSync('git', ['-C', repositoryRoot, 'status', '--porcelain', '--untracked-files=normal'], {
      encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'ignore']
    });
    sourceClean = status.trim().length === 0;
  }
  if (!hasEnvironmentProposalIds) {
    const message = execFileSync('git', ['-C', repositoryRoot, 'show', '-s', '--format=%B', 'HEAD'], {
      encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'ignore']
    });
    toolEvolutionProposalIds = toolEvolutionProposalIdsFromCommitMessage(message);
  }
} catch {
  // Builds without Git remain supported, but runtime trust stays unknown unless provenance was injected.
}
if (sourceClean !== true) toolEvolutionProposalIds = [];

const target = path.join(packageRoot, 'dist', 'build-info.json');
await mkdir(path.dirname(target), { recursive: true });
await writeFile(target, `${JSON.stringify({
  schemaVersion: 1,
  buildGitSha,
  sourceClean,
  toolEvolutionProposalIds
}, null, 2)}\n`, 'utf8');
