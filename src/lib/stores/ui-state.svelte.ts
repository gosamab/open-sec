/**
 * UI-only state — selection, filter, view mode, the persisted
 * `hide-dismissed` preference. None of this depends on scan results;
 * a fresh load with no scan still has all of these initialised.
 */

import { DEFAULT_FINDINGS_FILTER, type FindingsFilter } from '$lib/scan-display';

const HIDE_DISMISSED_KEY = 'open-sec:hide-dismissed';

function readHideDismissed(): boolean {
	if (typeof window === 'undefined') return true;
	return window.localStorage.getItem(HIDE_DISMISSED_KEY) !== 'false';
}

export type View = 'launcher' | 'workspace';

class UiState {
	view = $state<View>('launcher');
	settingsOpen = $state(false);

	// Findings selection / filtering
	selectedFile = $state<string | null>(null);
	selectedFindingId = $state<string | null>(null);
	filter = $state('');
	filterConfig = $state<FindingsFilter>({ ...DEFAULT_FINDINGS_FILTER });

	// File tree
	expandedFolders = $state<Set<string>>(new Set());

	// Persisted UI preference
	hideDismissed = $state<boolean>(readHideDismissed());

	/** Reset selection + filter (called when entering a new project / workspace). */
	resetSelection() {
		this.selectedFile = null;
		this.selectedFindingId = null;
		this.filter = '';
		this.filterConfig = { ...DEFAULT_FINDINGS_FILTER };
	}

	/** Persist `hideDismissed` to localStorage. Call from a `$effect` so it
	 *  runs whenever the value changes. */
	persistHideDismissed() {
		if (typeof window === 'undefined') return;
		try {
			window.localStorage.setItem(HIDE_DISMISSED_KEY, String(this.hideDismissed));
		} catch {
			// quota / disabled — silent
		}
	}
}

export const ui = new UiState();
