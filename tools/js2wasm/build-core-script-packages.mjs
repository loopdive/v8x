// Build-side inventory and AOT packaging of unchanged Deno Rust test inputs.
// Nonliteral inputs and compile errors remain explicit rows, never successes.
import assert from "node:assert/strict";
import {execFileSync} from "node:child_process";
import {createHash,randomUUID} from "node:crypto";
import {mkdirSync,readFileSync,writeFileSync} from "node:fs";
import {join,resolve} from "node:path";
import {literalScripts} from "./rust-script-literals.mjs";
import {packageScript} from "./script-packages.mjs";

const [compiler, precompiler, deno, output, ...paths] = process.argv.slice(2);
assert(output && paths.length, "usage: build-core-script-packages.mjs JS2 PACKAGER DENO OUTPUT GIT_PATH...");
const env = {...process.env};
delete env.GIT_DIR;
delete env.GIT_WORK_TREE;
const gitAt = (root,args) => execFileSync("git",["-C",resolve(root),...args],{encoding:"utf8",env});
const git = args => gitAt(deno,args);
const revision = git(["rev-parse","HEAD"]).trim();
assert.equal(revision,"1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44");
mkdirSync(resolve(output),{recursive:true});
const compilerDiff = gitAt(compiler,["diff","HEAD","--","src"]);
assert.equal(gitAt(compiler,["ls-files","--others","--exclude-standard","--","src"]).trim(),"",
  "commit untracked compiler sources before packaging");
const report = {runId:randomUUID(),revision, compiler:{revision:gitAt(compiler,["rev-parse","HEAD"]).trim(),
  trackedSourceDirty:compilerDiff.length>0,
  trackedSourceDiffSha256:createHash("sha256").update(compilerDiff).digest("hex")},
  precompilerSha256:createHash("sha256").update(readFileSync(resolve(precompiler))).digest("hex"),
  files:[], rows:[]};
const save = () => {
  const bytes=JSON.stringify(report,null,2)+"\n";
  writeFileSync(join(resolve(output),`core-inputs.${report.runId}.json`),bytes);
  writeFileSync(join(resolve(output),"core-inputs.json"),bytes);
};
for(const path of paths){
  const rust=git(["show",`HEAD:${path}`]);
  const inputs=literalScripts(rust,{inventory:true});
  assert(inputs.length>0,`no execute_script calls found in ${path}`);
  report.files.push({path,rustSha256:createHash("sha256").update(rust).digest("hex"),calls:inputs.length});
  for(const input of inputs){
    const row={path,...input};
    report.rows.push(row);
    if(input.unresolved){row.status="unresolved";save();console.log(path,input.line,row.status,input.unresolved);continue;}
    try{
      row.artifact=await packageScript(compiler,precompiler,input.specifier,input.source,output);
      row.status="packaged";
    }catch(error){row.status="packaging_failed";row.error=error.message;}
    save();
    console.log(path,input.line,row.status);
  }
}
const counts={};
for(const row of report.rows)counts[row.status]=(counts[row.status]??0)+1;
console.log(JSON.stringify({total:report.rows.length,counts}));
if(report.rows.some(row=>row.status!=="packaged"))process.exitCode=1;
