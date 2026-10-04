// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import { count } from "./shared.ts";
export const observed: number = count + 3;
export function read(): number { return count + 3; }
