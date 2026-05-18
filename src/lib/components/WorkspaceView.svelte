<script lang="ts">
	import ApiKeyPrompt from '$lib/components/ApiKeyPrompt.svelte';
	import FileStatusDetail from '$lib/components/FileStatusDetail.svelte';
	import FileTree from '$lib/components/FileTree.svelte';
	import FindingDetail from '$lib/components/FindingDetail.svelte';
	import FindingsList from '$lib/components/FindingsList.svelte';
	import OnboardingPanel from '$lib/components/OnboardingPanel.svelte';
	import PipelineProgress from '$lib/components/PipelineProgress.svelte';
	import ScanSummary from '$lib/components/ScanSummary.svelte';
	import WorkspaceTopBar from '$lib/components/WorkspaceTopBar.svelte';
	import { scan } from '$lib/stores/scan-state.svelte';
	import { triage } from '$lib/stores/triage-state.svelte';
	import { ui } from '$lib/stores/ui-state.svelte';
	import type { Excerpt, Finding, Patch, Severity, StageUsage, Verdict } from '$lib/ipc';
	import type {
		FindingStatus,
		FindingStatusInputs
	} from '$lib/scan-display';
	import type { FileNode, VisibleRow } from '$lib/tree';
	import type { TriageStatus } from '$lib/ipc';

	interface Props {
		// Status / derived banners
		humanizedError: { title: string; detail?: string } | null;
		showProgress: boolean;
		currentStageIndex: number;
		showOnboarding: boolean;

		// Derived view models
		statusInputs: FindingStatusInputs;
		visibleTree: VisibleRow[];
		totalFileNodes: number;
		totals: Record<FindingStatus, number>;
		visibleFindings: { rel: string; f: Finding }[];
		allFindings: { rel: string; f: Finding }[];
		dismissedCount: number;
		selectedFileNode: FileNode | null;
		selectedFinding: Finding | null;
		selectedFileNodeIsStatus: boolean;
		selectedVerdict: Verdict | null;
		selectedPatch: Patch | null;
		selectedPatchVariants: Patch[];
		selectedPatchVariantIdx: number;
		excerpt: Excerpt | null;
		excerptHtml: string | null;
		excerptError: string | null;
		diffHtml: string | null;
		severityCounts: Record<Severity, number>;
		usageRows: { name: string; u: StageUsage['triage']; ms: number }[];
		totalTokens: number;
		snoozeDays: number;

		// Callbacks (per-route; can't live in a store)
		onBack: () => void;
		onScan: () => void;
		onCancel: () => void;
		onOpenSettings: () => void;
		onExportMarkdown: () => void;
		onExportSarif: () => void;
		onRefreshKeyState: () => void;
		onSelectFile: (rel: string | null) => void;
		onSelectFinding: (id: string) => void;
		onToggleFolder: (p: string) => void;
		onApplyTriage: (findingId: string, status: TriageStatus, reason?: string) => void;
		onClearTriage: (findingId: string) => void;
		onStartDismiss: (findingId: string) => void;
		onCancelDismiss: () => void;
		onSubmitDismiss: (findingId: string) => void;
		onApplyPatch: () => void;
		onRegenerate: () => void;
		onSelectVariant: (idx: number) => void;
		onRetryDetect: (rel: string) => void;
		onRetryAll: () => void;
		onClearSelection: () => void;
	}

	let {
		humanizedError,
		showProgress,
		currentStageIndex,
		showOnboarding,
		statusInputs,
		visibleTree,
		totalFileNodes,
		totals,
		visibleFindings,
		allFindings,
		dismissedCount,
		selectedFileNode,
		selectedFinding,
		selectedFileNodeIsStatus,
		selectedVerdict,
		selectedPatch,
		selectedPatchVariants,
		selectedPatchVariantIdx,
		excerpt,
		excerptHtml,
		excerptError,
		diffHtml,
		severityCounts,
		usageRows,
		totalTokens,
		snoozeDays,
		onBack,
		onScan,
		onCancel,
		onOpenSettings,
		onExportMarkdown,
		onExportSarif,
		onRefreshKeyState,
		onSelectFile,
		onSelectFinding,
		onToggleFolder,
		onApplyTriage,
		onClearTriage,
		onStartDismiss,
		onCancelDismiss,
		onSubmitDismiss,
		onApplyPatch,
		onRegenerate,
		onSelectVariant,
		onRetryDetect,
		onRetryAll,
		onClearSelection
	}: Props = $props();
</script>

<div class="flex h-screen flex-col">
	<WorkspaceTopBar
		root={scan.root}
		scanning={scan.scanning}
		cancelling={scan.cancelling}
		keyConfigured={scan.keyConfigured}
		scanResult={scan.scanResult}
		resultRoot={scan.resultRoot}
		stage={scan.stage}
		{onBack}
		{onScan}
		{onCancel}
		{onOpenSettings}
		{onExportMarkdown}
		{onExportSarif}
	/>

	{#if !scan.keyConfigured}
		<ApiKeyPrompt variant="strip" onSaved={onRefreshKeyState} />
	{/if}

	{#if humanizedError}
		<div class="border-b border-destructive/40 bg-destructive/5 px-4 py-2 text-xs">
			<div class="flex items-baseline gap-2">
				<span class="font-medium text-destructive">{humanizedError.title}</span>
				{#if humanizedError.detail}
					<span class="text-destructive/80">— {humanizedError.detail}</span>
				{/if}
			</div>
		</div>
	{/if}

	{#if showProgress}
		<PipelineProgress
			stageIndex={currentStageIndex}
			stage={scan.stage}
			rateLimitNotice={scan.rateLimitNotice}
			durations={scan.durations}
		/>
	{/if}

	{#if showOnboarding}
		<OnboardingPanel root={scan.root} keyConfigured={scan.keyConfigured} {onScan} />
	{:else}
		<div
			class="grid flex-1 grid-cols-[260px_minmax(320px,1fr)_minmax(400px,1.4fr)] overflow-hidden"
		>
			<FileTree
				{visibleTree}
				{totalFileNodes}
				totalFindings={totals.open +
					totals.patched +
					totals.accepted +
					totals.snoozed +
					totals.dismissed +
					totals.dropped +
					totals.pending +
					totals.verifying}
				selectedFile={ui.selectedFile}
				scanning={scan.scanning}
				stage={scan.stage}
				hasWalk={!!scan.walk}
				walkCandidateCount={scan.walk?.candidates.length ?? 0}
				hasTriaged={scan.triaged.length > 0}
				expandedFolders={ui.expandedFolders}
				{onSelectFile}
				{onToggleFolder}
			/>

			<FindingsList
				{visibleFindings}
				allFindingsCount={allFindings.length}
				bind:filter={ui.filter}
				bind:hideDismissed={ui.hideDismissed}
				{dismissedCount}
				bind:filterConfig={ui.filterConfig}
				selectedFindingId={ui.selectedFindingId}
				selectedFile={ui.selectedFile}
				{selectedFileNode}
				scanning={scan.scanning}
				stage={scan.stage}
				hasWalk={!!scan.walk}
				walkCandidateCount={scan.walk?.candidates.length ?? 0}
				detectErrors={scan.detectErrors}
				{statusInputs}
				{onSelectFinding}
				{onSelectFile}
			/>

			<section class="flex flex-col overflow-hidden">
				<div class="flex h-10 items-center justify-between border-b border-border px-3">
					<span class="text-xs font-medium tracking-wide text-muted-foreground uppercase">
						{selectedFinding
							? 'Finding detail'
							: selectedFileNodeIsStatus
								? 'File status'
								: 'Summary'}
					</span>
					{#if selectedFinding || selectedFileNodeIsStatus}
						<button
							type="button"
							class="inline-flex h-6 items-center gap-1 rounded px-2 text-[0.6875rem] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
							title="Back to summary (Esc)"
							aria-label="Back to summary"
							onclick={onClearSelection}
						>
							<svg
								xmlns="http://www.w3.org/2000/svg"
								width="10"
								height="10"
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2.5"
								stroke-linecap="round"
								stroke-linejoin="round"
							>
								<path d="M18 6 6 18" />
								<path d="m6 6 12 12" />
							</svg>
							<span>Summary</span>
						</button>
					{/if}
				</div>
				<div class="flex-1 overflow-y-auto">
					{#if selectedFinding}
						<FindingDetail
							finding={selectedFinding}
							verdict={selectedVerdict}
							hasVerdictKey={scan.verdictById.has(selectedFinding.id)}
							patch={selectedPatch}
							patchVariants={selectedPatchVariants}
							patchVariantIdx={selectedPatchVariantIdx}
							triageRecord={triage.triageById.get(selectedFinding.id) ?? null}
							applied={triage.appliedPatchIds.has(selectedFinding.id)}
							dismissDraftActive={triage.dismissDraftFor === selectedFinding.id}
							bind:dismissReason={triage.dismissReason}
							triageBusy={triage.triageBusy}
							applyBusy={triage.applyBusy}
							applyError={triage.applyError}
							regenBusy={triage.regenBusy}
							regenError={triage.regenError}
							{excerpt}
							{excerptHtml}
							{excerptError}
							{diffHtml}
							scanning={scan.scanning}
							{statusInputs}
							{snoozeDays}
							onApplyTriage={(status, reason) => {
								if (selectedFinding) onApplyTriage(selectedFinding.id, status, reason);
							}}
							onClearTriage={() => {
								if (selectedFinding) onClearTriage(selectedFinding.id);
							}}
							onStartDismiss={() => {
								if (selectedFinding) onStartDismiss(selectedFinding.id);
							}}
							{onCancelDismiss}
							onSubmitDismiss={() => {
								if (selectedFinding) onSubmitDismiss(selectedFinding.id);
							}}
							{onApplyPatch}
							{onRegenerate}
							{onSelectVariant}
						/>
					{:else if selectedFileNode && selectedFileNodeIsStatus}
						<FileStatusDetail node={selectedFileNode} />
					{:else}
						<ScanSummary
							scanResult={scan.scanResult}
							scanning={scan.scanning}
							stage={scan.stage}
							keyConfigured={scan.keyConfigured}
							root={scan.root}
							walk={scan.walk}
							patchCount={scan.patchById.size}
							allFindingsTotal={allFindings.length}
							{severityCounts}
							{totals}
							durations={scan.durations}
							usage={scan.usage}
							{usageRows}
							{totalTokens}
							{totalFileNodes}
							detectErrors={scan.detectErrors}
							retryingFiles={scan.retryingFiles}
							retryingAll={scan.retryingAll}
							onRunScan={onScan}
							{onSelectFile}
							{onRetryDetect}
							{onRetryAll}
						/>
					{/if}
				</div>
			</section>
		</div>
	{/if}
</div>
