import assert from "node:assert/strict";
import { splitFencedCode } from "../src/lib/markdown.ts";

// Unit check for the chat body splitter behind BB-206. Fenced blocks become
// code segments; everything else stays exact text so @mention highlighting and
// line breaks survive. Node strips the TypeScript types on import.
//
// Run: node apps/web/tests/markdown.static.mjs (from the repo root)

const types = (segments) => segments.map((s) => s.type);

assert.deepEqual(splitFencedCode("plain text"), [{ type: "text", text: "plain text" }]);
assert.deepEqual(splitFencedCode(""), []);
assert.deepEqual(types(splitFencedCode("a\n\nb")), ["text"], "blank lines stay in the prose");

const fenced = splitFencedCode("before\n```rust\nfn main() {}\n```\nafter");
assert.deepEqual(types(fenced), ["text", "code", "text"]);
assert.equal(fenced[1].text, "fn main() {}");
assert.equal(fenced[1].lang, "rust");
assert.equal(fenced[0].text, "before", "the fence line is not part of the prose");
assert.equal(fenced[2].text, "after", "text after the fence is preserved");

const untagged = splitFencedCode("```\nls -la\n```");
assert.deepEqual(untagged, [{ type: "code", text: "ls -la", lang: null }]);

const untaggedSpaced = splitFencedCode("```  \nls -la\n```  ");
assert.deepEqual(untaggedSpaced, [{ type: "code", text: "ls -la", lang: null }]);

const two = splitFencedCode("```sh\ncargo test\n```\nbetween\n```rs\nfn x() {}\n```");
assert.deepEqual(types(two), ["code", "text", "code"]);
assert.equal(two[1].text, "between");

const open = splitFencedCode("look:\n```python\nprint('hi')\nstill code");
assert.deepEqual(types(open), ["text", "code"], "an unterminated fence runs to the end");
assert.equal(open[1].lang, "python");
assert.equal(open[1].text, "print('hi')\nstill code");

const longFence = splitFencedCode("````md\n```\ninner\n```\n````");
assert.deepEqual(longFence, [{ type: "code", text: "```\ninner\n```", lang: "md" }], "four-backtick fences wrap a fence");

const empty = splitFencedCode("```\n```");
assert.deepEqual(empty, [{ type: "code", text: "", lang: null }]);

const indented = splitFencedCode("  ```js\n  const x = 1;\n  ```");
assert.equal(indented.length, 1);
assert.equal(indented[0].type, "code");
assert.equal(indented[0].lang, "js");
assert.equal(indented[0].text, "  const x = 1;", "code indentation is preserved");

console.log("chat markdown splitter: passed");
