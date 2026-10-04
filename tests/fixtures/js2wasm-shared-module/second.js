import * as shared from "./shared.js";
import { count as namedCount } from "./shared.js";
export const same = shared === globalThis.firstSharedNamespace;
export const observed = shared.count;
export const runs = globalThis.sharedModuleRuns;
export { shared };
export function read() { return shared.count; }
export function readNamed() { return namedCount; }
