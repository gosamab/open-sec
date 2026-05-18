/**
 * Triage + patch-application state: user decisions persisted in the SQLite
 * store, plus the per-action "busy" flags and ephemeral dismiss-draft
 * fields used by the finding-detail UI.
 */

import type { TriageRecord } from '$lib/ipc';

class TriageState {
	// Persisted decisions keyed by finding_id (loaded from SQLite on project open).
	triageById = $state<Map<string, TriageRecord>>(new Map());
	triageBusy = $state(false);

	// Applied-patch ledger
	appliedPatchIds = $state<Set<string>>(new Set());
	applyBusy = $state(false);
	applyError = $state<string | null>(null);

	// Patch regeneration (busy + error). The patch history map itself lives
	// in scan-state because it's a result-shaped Map keyed by finding_id.
	regenBusy = $state(false);
	regenError = $state<string | null>(null);

	// In-flight "dismiss with reason" draft. `dismissDraftFor` is the
	// finding_id currently showing the reason input; `dismissReason` is the
	// uncommitted text. Both clear on submit / cancel / selection change.
	dismissDraftFor = $state<string | null>(null);
	dismissReason = $state('');
}

export const triage = new TriageState();
