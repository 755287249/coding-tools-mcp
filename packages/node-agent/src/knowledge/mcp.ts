import { createHash } from 'node:crypto';
import type { JsonObject, ToolContext } from '../types.js';
import type { KnowledgeRecord, ToolEvolutionChangeProposalRecord } from './types.js';

const RESOURCE_PREFIX = 'tool-evolution://coding-tools/';

interface ProposalEntry {
  folderId: string;
  folderName: string;
  proposal: ToolEvolutionChangeProposalRecord;
  knowledge: KnowledgeRecord;
  proposalRevision: string;
  knowledgeRevision: string;
}

function encodePart(value: string): string {
  return encodeURIComponent(value);
}

function decodePart(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    throw Object.assign(new Error('Invalid Tool Evolution resource identifier.'), { rpcCode: -32602 });
  }
}

export function toolEvolutionResourceUri(folderId: string, proposalId: string): string {
  return `${RESOURCE_PREFIX}${encodePart(folderId)}/${encodePart(proposalId)}`;
}

export function isToolEvolutionResourceUri(uri: string): boolean {
  return uri.startsWith(RESOURCE_PREFIX);
}

function parseResourceUri(uri: string): { folderId: string; proposalId: string } | undefined {
  if (!isToolEvolutionResourceUri(uri)) return undefined;
  const parts = uri.slice(RESOURCE_PREFIX.length).split('/');
  if (parts.length !== 2 || !parts[0] || !parts[1]) return undefined;
  return { folderId: decodePart(parts[0]), proposalId: decodePart(parts[1]) };
}

function evidenceSummary(record: KnowledgeRecord): JsonObject {
  return {
    observations: record.evidence.observations,
    successes: record.evidence.successes,
    failures: record.evidence.failures,
    confidence: record.confidence,
    task_context_count: record.evidence.taskContextIds?.length ?? 0,
    conversation_context_count: record.evidence.conversationContextIds?.length ?? 0,
    runtime_boot_context_count: record.evidence.runtimeBootContextIds?.length ?? 0,
    first_seen_at_ms: record.evidence.firstSeenAtMs,
    last_seen_at_ms: record.evidence.lastSeenAtMs
  };
}

function proposalMeta(entry: ProposalEntry): JsonObject {
  return {
    'coding-tools/resource-kind': 'tool-evolution-proposal',
    'coding-tools/workspace-folder-id': entry.folderId,
    'coding-tools/workspace-folder-name': entry.folderName,
    'coding-tools/tool-evolution-proposal-id': entry.proposal.proposalId,
    'coding-tools/tool-evolution-tool': entry.proposal.tool,
    'coding-tools/tool-evolution-proposal-type': entry.proposal.proposalType,
    'coding-tools/tool-evolution-base-revision': entry.proposal.baseRevision,
    'coding-tools/tool-evolution-proposal-revision': entry.proposalRevision,
    'coding-tools/knowledge-revision': entry.knowledgeRevision
  };
}

async function entries(ctx: ToolContext): Promise<{ entries: ProposalEntry[]; revision: string }> {
  const output: ProposalEntry[] = [];
  const revisionMaterial: Array<{ folderId: string; proposalRevision: string; knowledgeRevision: string }> = [];
  const folders = [...ctx.config.folders].sort((left, right) => left.id.localeCompare(right.id));
  for (const folder of folders) {
    const runtime = ctx.folderRuntimes.get(folder.id);
    if (!runtime) continue;
    const [proposals, proposalRevision, knowledgeRevision] = await Promise.all([
      runtime.toolEvolutionProposalStore.list(),
      runtime.toolEvolutionProposalStore.revision(),
      runtime.knowledgeStore.revision()
    ]);
    revisionMaterial.push({ folderId: folder.id, proposalRevision, knowledgeRevision });
    for (const proposal of proposals) {
      if (proposal.status !== 'proposed') continue;
      const knowledge = await runtime.knowledgeStore.get(proposal.knowledgeId);
      if (!knowledge || knowledge.target !== 'tool_evolution') continue;
      output.push({
        folderId: folder.id,
        folderName: folder.name,
        proposal,
        knowledge,
        proposalRevision,
        knowledgeRevision
      });
    }
  }
  output.sort((left, right) => left.folderId.localeCompare(right.folderId)
    || left.proposal.tool.localeCompare(right.proposal.tool)
    || left.proposal.proposalId.localeCompare(right.proposal.proposalId));
  return {
    entries: output,
    revision: createHash('sha256').update(JSON.stringify(revisionMaterial)).digest('hex')
  };
}

async function findEntry(ctx: ToolContext, folderId: string, proposalId: string): Promise<ProposalEntry | undefined> {
  const folder = ctx.config.folders.find(candidate => candidate.id === folderId);
  const runtime = ctx.folderRuntimes.get(folderId);
  if (!folder || !runtime) return undefined;
  const [proposal, proposalRevision, knowledgeRevision] = await Promise.all([
    runtime.toolEvolutionProposalStore.get(proposalId),
    runtime.toolEvolutionProposalStore.revision(),
    runtime.knowledgeStore.revision()
  ]);
  if (!proposal || proposal.status !== 'proposed') return undefined;
  const knowledge = await runtime.knowledgeStore.get(proposal.knowledgeId);
  if (!knowledge || knowledge.target !== 'tool_evolution') return undefined;
  return {
    folderId,
    folderName: folder.name,
    proposal,
    knowledge,
    proposalRevision,
    knowledgeRevision
  };
}

export async function listToolEvolutionResources(ctx: ToolContext): Promise<JsonObject> {
  const catalog = await entries(ctx);
  return {
    resources: catalog.entries.map(entry => ({
      uri: toolEvolutionResourceUri(entry.folderId, entry.proposal.proposalId),
      name: `${entry.proposal.tool}-evolution-${entry.proposal.proposalId.slice(0, 12)}`,
      title: `Tool Evolution: ${entry.proposal.tool} — ${entry.folderName}`,
      description: entry.knowledge.hypothesis,
      mimeType: 'application/json',
      _meta: proposalMeta(entry)
    })),
    _meta: { 'coding-tools/tool-evolution-resource-revision': catalog.revision }
  };
}

export async function readToolEvolutionResource(ctx: ToolContext, uri: string): Promise<JsonObject> {
  const parsed = parseResourceUri(uri);
  if (!parsed) throw Object.assign(new Error(`Resource not found: ${uri}`), { rpcCode: -32002 });
  const entry = await findEntry(ctx, parsed.folderId, parsed.proposalId);
  if (!entry) throw Object.assign(new Error(`Resource not found: ${uri}`), { rpcCode: -32002 });
  const document = {
    schema_version: 1,
    kind: 'tool_evolution_proposal',
    workspace: {
      folder_id: entry.folderId,
      name: entry.folderName
    },
    proposal: {
      id: entry.proposal.proposalId,
      status: entry.proposal.status,
      tool: entry.proposal.tool,
      proposal_type: entry.proposal.proposalType,
      base_revision: entry.proposal.baseRevision,
      created_at_ms: entry.proposal.createdAtMs,
      updated_at_ms: entry.proposal.updatedAtMs
    },
    hypothesis: entry.knowledge.hypothesis,
    recommended_action: entry.proposal.action,
    evidence: evidenceSummary(entry.knowledge),
    implementation_contract: {
      isolated_worktree_required: true,
      candidate_commit_required: true,
      required_commit_trailer: `Tool-Evolution-Proposal: ${entry.proposal.proposalId}`,
      base_revision_must_match: entry.proposal.baseRevision,
      preserve_static_security_boundaries: true,
      no_automatic_promotion: true,
      benchmark_before_promotion: true,
      validation_objective: 'preserve verified task quality while reducing calls, latency, or response cost'
    }
  };
  return {
    contents: [{ uri, mimeType: 'application/json', text: `${JSON.stringify(document, null, 2)}\n` }],
    _meta: proposalMeta(entry)
  };
}
