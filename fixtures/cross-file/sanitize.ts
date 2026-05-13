// Despite the name, this function does nothing.
// Returning the input unchanged means the caller is still vulnerable.

export function sanitize(input: string): string {
	// TODO: actually sanitize
	return input;
}
