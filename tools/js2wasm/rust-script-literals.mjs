// Read literal execute_script inputs for offline conformance packaging.
// Unsupported expressions fail loudly; they are not omitted or rewritten.
export function literalScripts(rust) {
  const calls = [...rust.matchAll(/\.execute_script\s*\(/g)];
  const whitespace = index => { while (/\s/.test(rust[index] ?? "") && index < rust.length) index++; return index; };
  function string(index) {
    index = whitespace(index);
    const raw = /^r(#+)?"/.exec(rust.slice(index));
    if (raw) {
      const start = index + raw[0].length;
      const close = '"' + (raw[1] ?? "");
      const end = rust.indexOf(close, start);
      if (end < 0) throw new Error("unterminated Rust raw string");
      return { value: rust.slice(start, end), next: end + close.length };
    }
    if (rust[index] !== '"') throw new Error("execute_script input is not a literal string");
    let end = index + 1;
    for (; end < rust.length; end++) {
      if (rust[end] === "\\") { end++; continue; }
      if (rust[end] === '"') break;
    }
    if (end === rust.length) throw new Error("unterminated Rust string");
    return { value: JSON.parse(rust.slice(index, end + 1)), next: end + 1 };
  }
  return calls.map(call => {
    const specifier = string(call.index + call[0].length);
    let index = whitespace(specifier.next);
    if (rust[index++] !== ",") throw new Error("missing execute_script argument separator");
    const source = string(index);
    index = whitespace(source.next);
    if (rust[index] === ",") index = whitespace(index + 1);
    if (rust[index] !== ")") throw new Error("execute_script source contains an unsupported expression");
    return { specifier: specifier.value, source: source.value,
      line: rust.slice(0, call.index).split("\n").length };
  });
}
