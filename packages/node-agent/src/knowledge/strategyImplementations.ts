import { createHash } from 'node:crypto';
import type { JsonObject } from '../types.js';
import type { KnowledgeRecord } from './types.js';

export type ToolStrategyImplementationId = 'search_exact_total_fast_tail';
export type ToolStrategyImplementationSafety = 'semantics_preserving';

export interface ToolStrategyImplementationSpec {
  id: ToolStrategyImplementationId;
  tool: string;
  safety: ToolStrategyImplementationSafety;
  matches(record: KnowledgeRecord, args: JsonObject): boolean;
}

const IMPLEMENTATIONS: readonly ToolStrategyImplementationSpec[] = [
  {
    id: 'search_exact_total_fast_tail',
    tool: 'search_text',
    safety: 'semantics_preserving',
    matches(record, args) {
      const declaredImplementation = record.recommendedAction.semantic_preserving_implementation;
      const legacyBoundedSearch = record.recommendedAction.recommendation === 'prefer_bounded_search';
      return record.trigger.tool === 'search_text'
        && (declaredImplementation === 'search_exact_total_fast_tail' || legacyBoundedSearch)
        && args.calculate_total === true
        && args.count_only !== true
        && (typeof args.query === 'string' || (Array.isArray(args.queries) && args.queries.length > 0));
    }
  }
] as const;

function stableDescriptor(spec: ToolStrategyImplementationSpec) {
  return { id: spec.id, tool: spec.tool, safety: spec.safety };
}

export function resolveToolStrategyImplementation(
  record: KnowledgeRecord,
  tool: string,
  args: JsonObject
): ToolStrategyImplementationSpec | undefined {
  if (record.target !== 'tool_strategy') return undefined;
  return IMPLEMENTATIONS.find(spec => spec.tool === tool
    && spec.safety === 'semantics_preserving'
    && spec.matches(record, args));
}

export function toolStrategyImplementationSnapshot(): {
  count: number;
  revision: string;
  implementations: Array<{ id: ToolStrategyImplementationId; tool: string; safety: ToolStrategyImplementationSafety }>;
} {
  const implementations = IMPLEMENTATIONS.map(stableDescriptor);
  const revision = createHash('sha256').update(JSON.stringify(implementations)).digest('hex');
  return { count: implementations.length, revision, implementations };
}
