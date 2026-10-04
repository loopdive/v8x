globalThis.sharedModuleRuns = (globalThis.sharedModuleRuns || 0) + 1;
export let count = 1;
export function bump() { return ++count; }
export function sum(a, b) { count += a + b; return count; }
export function receiver() { return this; }
export const nil = null;
export const nonCallable = 1;
export const nested = Object.create(null);
nested.receiver = receiver;
