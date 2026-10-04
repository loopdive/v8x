import { value } from "custom:native";
import * as native from "custom:native";
export const observed = value;
export function live() { return value; }
export function namespace() { return native; }
