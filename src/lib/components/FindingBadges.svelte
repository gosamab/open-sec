<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import type { Finding } from '$lib/ipc';
	import {
		findingStatus,
		severityClass,
		statusClass,
		statusLabelFor,
		type FindingStatusInputs
	} from '$lib/scan-display';

	interface Props {
		finding: Finding;
		statusInputs: FindingStatusInputs;
		/** Include the kind badge (vuln / hardening). On in the detail header,
		 *  off in the list row where space is tight. */
		showKind?: boolean;
	}

	let { finding, statusInputs, showKind = false }: Props = $props();
	let status = $derived(findingStatus(finding, statusInputs));
</script>

<Badge class={severityClass(finding.severity)}>{finding.severity}</Badge>
<Badge class={statusClass(status)}>{statusLabelFor(finding, statusInputs)}</Badge>
{#if showKind}
	<Badge variant="outline">{finding.kind}</Badge>
{/if}
