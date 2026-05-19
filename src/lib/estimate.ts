/**
 * Pre-scan cost estimate from a {@link WalkResult}. Pure heuristic — no LLM
 * is called. The walk has already counted lines per candidate and listed
 * pre-triage skips, so estimates only price the files that will actually
 * reach the model.
 *
 * The heuristic is rough by design: we just want the user to know whether a
 * scan will cost cents or dollars before they commit. Numbers are calibrated
 * against typical scans on small/medium codebases (~5k LoC); actuals can
 * land within roughly ±50% depending on how detect's tool loop iterates and
 * how many findings verify/patch produce.
 */
import type { ScanSettings } from './settings.svelte';
import type { WalkResult } from './ipc';
import { priceFor, throughputFor, type StageCosts } from './pricing';

/** Rough tokens-per-line-of-code multiplier when feeding source to the model.
 *  Picks up real-source averages across TS/Python/Go/Rust within the
 *  tokenizer's BPE; varies by language but 6 is a decent middle. */
const TOKENS_PER_LOC = 6;

/** System / tool / instruction overhead per file at each stage. Triage's
 *  prompt is small; detect carries the full tool surface; verify and patch
 *  carry per-finding prompts. These are the "fixed cost" anchor each call. */
const TRIAGE_SYSTEM_TOKENS = 1500;
const DETECT_SYSTEM_TOKENS = 5000;
const VERIFY_SYSTEM_TOKENS = 3000;
const PATCH_SYSTEM_TOKENS = 4000;

/** Detect runs an agent loop with `read_file`/`grep`/etc., so the file's
 *  contents typically flow through the context more than once. */
const DETECT_FILE_READ_MULTIPLIER = 2;

/** Typical output sizes per file/finding (JSON payloads). */
const TRIAGE_OUTPUT_PER_FILE = 80;
const DETECT_OUTPUT_PER_FILE = 600;
const VERIFY_OUTPUT_PER_FINDING = 400;
const PATCH_OUTPUT_PER_FINDING = 600;

/** Yield rates: roughly what fraction of candidates produce findings, how
 *  many findings on average, and what fraction of findings get a patch.
 *  Calibrated from a handful of mid-sized scans — coarse but unbiased. */
const FILES_WITH_FINDINGS_RATE = 0.3;
const FINDINGS_PER_FLAGGED_FILE = 1.5;
const PATCH_RATE_OF_VERIFIED = 0.4;

/** Per-call wall-clock overhead (TLS handshake, queueing, server-side
 *  prefill). Triage is a single short call; detect runs an agent loop with
 *  multiple round trips; verify and patch are heavier single shots. */
const TRIAGE_OVERHEAD_SEC = 1.5;
const DETECT_OVERHEAD_SEC = 4;
const VERIFY_OVERHEAD_SEC = 2;
const PATCH_OVERHEAD_SEC = 2;

/** Detect runs `read_file` / `grep` / etc. tools — assume ~3 round trips
 *  per file on average. Each round trip adds an overhead + output stream. */
const DETECT_LOOP_ITERATIONS = 3;

export interface TimeEstimate {
	/** Estimated seconds spent on each stage (after concurrency). */
	triage_sec: number;
	detect_sec: number;
	verify_sec: number;
	patch_sec: number;
	/** Sum of per-stage seconds (stages run sequentially). */
	total_sec: number;
}

export interface CostEstimate {
	/** Total LoC across candidate files only (skipped files are excluded). */
	total_loc: number;
	/** Number of files the scan will send to the model. */
	candidate_files: number;
	/** Files dropped before triage (vendor dirs, too-large, binary, minified). */
	skipped_files: number;
	costs: StageCosts;
	time: TimeEstimate;
}

export function estimateScanCost(walk: WalkResult, settings: ScanSettings): CostEstimate {
	const candidates = walk.candidates;
	const totalLoc = candidates.reduce((n, c) => n + c.line_count, 0);
	const fileCount = candidates.length;

	if (fileCount === 0) {
		return {
			total_loc: 0,
			candidate_files: 0,
			skipped_files: walk.skipped.length,
			costs: { triage: 0, detect: 0, verify: 0, patch: 0, total: 0 },
			time: { triage_sec: 0, detect_sec: 0, verify_sec: 0, patch_sec: 0, total_sec: 0 }
		};
	}

	const avgFileLoc = totalLoc / fileCount;

	// Triage: every candidate gets one cheap call. Input = system + the file
	// itself. Output is small JSON.
	const triageInput = fileCount * (TRIAGE_SYSTEM_TOKENS + avgFileLoc * TOKENS_PER_LOC);
	const triageOutput = fileCount * TRIAGE_OUTPUT_PER_FILE;
	const triage = priceCost(triageInput, triageOutput, settings.triage_model);
	const triageTimePerCall =
		TRIAGE_OVERHEAD_SEC + TRIAGE_OUTPUT_PER_FILE / throughputFor(settings.triage_model);
	const triageSec = (fileCount * triageTimePerCall) / Math.max(1, settings.triage_concurrency);

	// Detect: same set of files, but the agent loop reads the file via tools,
	// often more than once. Larger system prompt (tool defs included).
	const detectInput =
		fileCount *
		(DETECT_SYSTEM_TOKENS + avgFileLoc * TOKENS_PER_LOC * DETECT_FILE_READ_MULTIPLIER);
	const detectOutput = fileCount * DETECT_OUTPUT_PER_FILE;
	const detect = priceCost(detectInput, detectOutput, settings.detect_model);
	// Detect's wall-clock is dominated by the agent loop's serial round trips.
	// Per file: each round trip is overhead + a slice of the total output.
	const detectTimePerFile =
		DETECT_LOOP_ITERATIONS * DETECT_OVERHEAD_SEC +
		DETECT_OUTPUT_PER_FILE / throughputFor(settings.detect_model);
	const detectSec = (fileCount * detectTimePerFile) / Math.max(1, settings.detect_concurrency);

	// Verify: only fires if detect found something. Estimate the expected
	// finding count and price one verify call per finding.
	const expectedFindings = fileCount * FILES_WITH_FINDINGS_RATE * FINDINGS_PER_FLAGGED_FILE;
	const verifyInput =
		expectedFindings * (VERIFY_SYSTEM_TOKENS + avgFileLoc * TOKENS_PER_LOC);
	const verifyOutput = expectedFindings * VERIFY_OUTPUT_PER_FINDING;
	const verify = priceCost(verifyInput, verifyOutput, settings.verify_model);
	const verifyTimePerCall =
		VERIFY_OVERHEAD_SEC + VERIFY_OUTPUT_PER_FINDING / throughputFor(settings.verify_model);
	const verifySec =
		(expectedFindings * verifyTimePerCall) / Math.max(1, settings.verify_concurrency);

	// Patch: only some verified findings reach the patcher.
	const expectedPatches = expectedFindings * PATCH_RATE_OF_VERIFIED;
	const patchInput =
		expectedPatches * (PATCH_SYSTEM_TOKENS + avgFileLoc * TOKENS_PER_LOC);
	const patchOutput = expectedPatches * PATCH_OUTPUT_PER_FINDING;
	const patch = priceCost(patchInput, patchOutput, settings.patch_model);
	const patchTimePerCall =
		PATCH_OVERHEAD_SEC + PATCH_OUTPUT_PER_FINDING / throughputFor(settings.patch_model);
	const patchSec =
		(expectedPatches * patchTimePerCall) / Math.max(1, settings.patch_concurrency);

	const total = triage + detect + verify + patch;
	const totalSec = triageSec + detectSec + verifySec + patchSec;

	return {
		total_loc: totalLoc,
		candidate_files: fileCount,
		skipped_files: walk.skipped.length,
		costs: { triage, detect, verify, patch, total },
		time: {
			triage_sec: triageSec,
			detect_sec: detectSec,
			verify_sec: verifySec,
			patch_sec: patchSec,
			total_sec: totalSec
		}
	};
}

function priceCost(inputTokens: number, outputTokens: number, model: string): number {
	const p = priceFor(model);
	// We don't try to estimate cache hits — that requires real run data, and
	// caching only ever lowers actual cost vs. the all-fresh-input estimate.
	return (inputTokens * p.input + outputTokens * p.output) / 1_000_000;
}
