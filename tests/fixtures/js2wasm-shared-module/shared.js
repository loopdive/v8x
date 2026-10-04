globalThis.sharedModuleRuns = (globalThis.sharedModuleRuns || 0) + 1;
export let count = 1;
export function bump() { return ++count; }
export function receiver() { return this; }
