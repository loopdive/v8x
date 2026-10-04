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
export function optionalNamed() { return namedBump?.(); }
export function optionalMethod() { return shared.bump?.(); }
export function optionalComputed() { return shared['bump']?.(); }
export function optionalNamespace() { return shared?.bump(); }
export function optionalReceiver() { return shared.receiver?.(); }
export function optionalNamedReceiver() { return namedReceiver?.(); }
export function absentCall() { return shared.absent?.(namedBump()); }
export function absentReceiver() { return shared.nothing?.[namedBump()](); }
export function chainSkip() { return shared.nothing?.foo.bar(namedBump()); }
export function computedSkip() { return shared.nothing?.[namedBump()].foo(namedBump()); }
export function nullSkip() { return shared.nil?.[namedBump()].foo(namedBump()); }
export function parenthesizedReceiver() { return (shared.receiver)?.(); }
export function nestedReceiver() { return shared.nested?.receiver?.(); }
export function parenBreak() { try { (shared.nothing?.foo).bar(namedBump()); } catch (error) { return 1; } return 0; }
export function nonCallable() { try { shared.nonCallable?.(namedBump()); } catch (error) { return 1; } return 0; }
