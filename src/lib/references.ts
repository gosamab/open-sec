/**
 * External references for a finding — CWE / OWASP / supplementary resources.
 * Pure URL builders. Callers render these as `<a>` tags inside a `.md`
 * container so they pick up the `data-md-link` external-link hardening
 * from [markdown.ts](markdown.ts).
 */

import type { Finding } from './ipc';

export interface Reference {
	label: string;
	url: string;
	/** Short hint shown next to the label (e.g. "MITRE", "OWASP"). */
	source: string;
}

/** OWASP Top-10 (2021) slugs. The Finding payload carries values like
 *  "A03:2021"; we need the published slug "A03_2021-Injection" to deep-link.
 *  Maintained by hand; the list changes only when OWASP cuts a new edition. */
const OWASP_2021_SLUGS: Record<string, string> = {
	'A01:2021': 'A01_2021-Broken_Access_Control',
	'A02:2021': 'A02_2021-Cryptographic_Failures',
	'A03:2021': 'A03_2021-Injection',
	'A04:2021': 'A04_2021-Insecure_Design',
	'A05:2021': 'A05_2021-Security_Misconfiguration',
	'A06:2021': 'A06_2021-Vulnerable_and_Outdated_Components',
	'A07:2021': 'A07_2021-Identification_and_Authentication_Failures',
	'A08:2021': 'A08_2021-Software_and_Data_Integrity_Failures',
	'A09:2021': 'A09_2021-Security_Logging_and_Monitoring_Failures',
	'A10:2021': 'A10_2021-Server-Side_Request_Forgery_%28SSRF%29'
};

/** Convert a CWE id (e.g. `CWE-89`, `cwe-89`, or just `89`) into the MITRE
 *  deep link. Returns `null` if the id can't be parsed. */
export function cweUrl(cwe: string | null | undefined): string | null {
	if (!cwe) return null;
	const m = cwe.trim().match(/(?:CWE[-_]?)?(\d+)/i);
	if (!m) return null;
	return `https://cwe.mitre.org/data/definitions/${m[1]}.html`;
}

/** Convert an OWASP code (e.g. `A03:2021`) into the published Top-10 deep
 *  link. Unknown codes fall back to the Top-10 index. */
export function owaspUrl(owasp: string | null | undefined): string | null {
	if (!owasp) return null;
	const trimmed = owasp.trim();
	const slug = OWASP_2021_SLUGS[trimmed];
	if (slug) return `https://owasp.org/Top10/${slug}/`;
	if (/^A\d+:\d{4}$/i.test(trimmed)) {
		// Recognized format, unknown year → land on the Top-10 index.
		return 'https://owasp.org/www-project-top-ten/';
	}
	return null;
}

/** Assemble the reference list for a finding. Order: CWE first, then OWASP. */
export function referencesFor(finding: Finding): Reference[] {
	const out: Reference[] = [];
	const cwe = cweUrl(finding.cwe);
	if (cwe) {
		out.push({ label: finding.cwe, url: cwe, source: 'MITRE' });
	}
	const owasp = owaspUrl(finding.owasp);
	if (owasp) {
		out.push({
			label: `OWASP ${finding.owasp}`,
			url: owasp,
			source: 'OWASP Top 10'
		});
	}
	return out;
}
