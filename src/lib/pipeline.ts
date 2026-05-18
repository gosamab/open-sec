/** The five pipeline stages, with display metadata for both the onboarding
 *  panel and the live progress bar. Keep this list aligned with
 *  `src-tauri/src/scanner/orchestrate.rs::run_scan` ordering. */
export interface PipelineStage {
	key: string;
	label: string;
	/** Which Anthropic model the stage uses. `null` for ingest (pure I/O). */
	model: string | null;
	/** Short verb-phrase for the onboarding cards ("Walk & filter", etc). */
	desc: string;
}

export const PIPELINE_STAGES: PipelineStage[] = [
	{ key: 'ingest', label: 'Ingest', model: null, desc: 'Walk & filter' },
	{ key: 'triage', label: 'Triage', model: 'Haiku', desc: 'Prioritize' },
	{ key: 'detect', label: 'Detect', model: 'Sonnet', desc: 'Find issues' },
	{ key: 'verify', label: 'Verify', model: 'Opus', desc: 'Confirm exploits' },
	{ key: 'patch', label: 'Patch', model: 'Sonnet', desc: 'Draft fixes' }
];

/** Map the orchestrator's free-form `stage` string into a stage index. */
export function stageIndex(stage: string): number {
	if (stage === 'idle' || stage === 'starting…') return -1;
	if (stage === 'scanning…') return 0;
	if (stage.startsWith('triaging')) return 1;
	if (stage.startsWith('detecting')) return 2;
	if (stage.startsWith('verifying')) return 3;
	if (stage.startsWith('proposing') || stage.startsWith('patching')) return 4;
	if (stage === 'done' || stage === 'cancelled') return PIPELINE_STAGES.length;
	return -1;
}
