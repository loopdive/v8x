globalThis.prefixRuns = (globalThis.prefixRuns || 0) + 1;
export let value = 7;
export function bump() { value++; }
export function snapshot() { return { value }; }
