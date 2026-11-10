import { marked } from "marked";
import DOMPurify from "dompurify";

marked.setOptions({ breaks: true, gfm: true });

/** Render markdown to sanitized HTML, safe to pass to `{@html ...}`. */
export function renderMarkdown(src: string): string {
  const html = marked.parse(src ?? "", { async: false }) as string;
  return DOMPurify.sanitize(html);
}

export type BodySegment =
  | { type: "text"; text: string }
  | { type: "code"; text: string; lang: string | null };

// A fence opens with three or more backticks (optionally indented) and an
// optional language tag. Four backticks are valid too, so match a run.
const OPEN_FENCE = /^\s*```+([^\s`]*)[^\n]*$/;
const CLOSE_FENCE = /^\s*```+\s*$/;

/**
 * Split a chat body into prose and fenced code segments.
 *
 * Chat bodies are not rendered as full markdown: exact whitespace and @mention
 * highlighting matter, and messages are untrusted text. Only fenced code is
 * lifted out, so prose keeps its line breaks and is rendered as text (never
 * `{@html}`). An unterminated fence runs to the end of the message: showing
 * the backticks would look more broken than an open block.
 */
export function splitFencedCode(src: string): BodySegment[] {
  const lines = (src ?? "").split("\n");
  const out: BodySegment[] = [];
  let text: string[] = [];
  let code: string[] | null = null;
  let lang: string | null = null;

  const flushText = () => {
    if (!text.length) return;
    out.push({ type: "text", text: text.join("\n") });
    text = [];
  };
  const flushCode = () => {
    if (code === null) return;
    out.push({ type: "code", text: code.join("\n"), lang });
    code = null;
    lang = null;
  };

  for (const line of lines) {
    if (code === null) {
      const open = OPEN_FENCE.exec(line);
      if (open) {
        flushText();
        code = [];
        lang = open[1] || null;
      } else {
        text.push(line);
      }
      continue;
    }
    if (CLOSE_FENCE.test(line)) {
      flushCode();
      continue;
    }
    code.push(line);
  }

  if (code !== null) flushCode();
  else flushText();
  return out;
}
