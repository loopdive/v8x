import * as shared from "./shared.js";
globalThis.firstSharedNamespace = shared;
export const initial = shared.bump();
export { shared };
