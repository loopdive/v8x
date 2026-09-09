// Diagnostic WasmGC enumeration probe, not a native Deno measurement.
// Run with node --experimental-wasm-exnref and two wasm-opt-optimized binaries.
import {readFileSync} from "node:fs";
import {createHash} from "node:crypto";
import {performance} from "node:perf_hooks";

// Compile this exact program with target:standalone and skipSemanticDiagnostics.
export const source = `
 let target:any = {};
 export function fill(n:number):void {target={};for(let i=0;i<n;i++)target["field"+i]=i;}
 export function scan():number {const keys=Reflect.ownKeys(target);let sum=0;for(let i=0;i<keys.length;i++)sum+=target[keys[i]];return sum;}
`;
const paths=process.argv.slice(2);
if(paths.length!==2)throw Error("usage: measure-own-key-sorting.mjs CONTROL.wasm CANDIDATE.wasm");
const artifacts=paths.map((path,index)=>{
  const binary=readFileSync(path), module=new WebAssembly.Module(binary);
  if(WebAssembly.Module.imports(module).length!==0)throw Error("unexpected imports");
  return {name:index===0?"control":"candidate",path,bytes:binary.length,
    sha256:createHash("sha256").update(binary).digest("hex"),exports:new WebAssembly.Instance(module,{}).exports};
});
const rows=[];
for(const n of [100,200,400,800,1600]) {
  for(const artifact of artifacts) {
    artifact.exports.fill(n);
    for(let warm=0;warm<2;warm++)if(artifact.exports.scan()!==n*(n-1)/2)throw Error("warmup checksum");
  }
  for(let round=0;round<5;round++)for(const artifact of round%2?[...artifacts].reverse():artifacts) {
    const start=performance.now();
    for(let repeat=0;repeat<5;repeat++)if(artifact.exports.scan()!==n*(n-1)/2)throw Error("checksum");
    rows.push({name:artifact.name,n,round,scans:5,ms:performance.now()-start});
  }
}
console.log(JSON.stringify({node:process.version,source,artifacts:artifacts.map(({exports,...record})=>record),rows},null,2));
