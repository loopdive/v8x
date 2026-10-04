import * as prefix from './prefix.js';
export const observed = prefix.value;
export function live() { return prefix.value; }
export { prefix };
