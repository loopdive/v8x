import * as shared from "./shared.js";
import { count as namedCount } from "./shared.js";
import { bump as namedBump, receiver as namedReceiver } from "./shared.js";
import { sum as namedSum } from "./shared.js";
export const same = shared === globalThis.firstSharedNamespace;
export const observed = shared.count;
export const runs = globalThis.sharedModuleRuns;
export { shared };
export function read() { return shared.count; }
export function readNamed() { return namedCount; }
export function mutateNamespace() { return shared.bump(); }
export function mutateNamed() { return namedBump(); }
export function receiverNamespace() { return shared.receiver(); }
export function receiverNamed() { return namedReceiver(); }
export function spreadNamespace() { const args = [2, 3]; return shared.sum(...args); }
export function spreadNamed() { const args = [2, 3]; return namedSum(...args); }
export function spreadMixed() { return namedSum(...[], 2, ...[3]); }
export function spreadNested() { return namedSum(...[namedBump(), namedBump()]); }
export function spreadInvalid() { try { namedSum(...null); } catch (error) { return 1; } return 0; }
