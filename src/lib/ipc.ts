import { invoke } from '@tauri-apps/api/core';

export type Severity = 'critical' | 'high' | 'medium' | 'low' | 'info';
export type FindingKind = 'vuln' | 'hardening';

export interface Finding {
	id: string;
	kind: FindingKind;
	severity: Severity;
	cwe: string;
	owasp: string | null;
	title: string;
	file: string;
	line_start: number;
	line_end: number;
	description: string;
	data_flow: string;
}

export async function greet(name: string): Promise<string> {
	return invoke<string>('greet', { name });
}

export async function hasAnthropicKey(): Promise<boolean> {
	return invoke<boolean>('has_anthropic_key');
}

export async function setAnthropicKey(key: string): Promise<void> {
	return invoke<void>('set_anthropic_key', { key });
}

export async function scanFile(path: string, scanRoot?: string): Promise<Finding[]> {
	return invoke<Finding[]>('scan_file', { path, scanRoot: scanRoot ?? null });
}
