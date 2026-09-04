import { marked } from "marked";
import DOMPurify from "dompurify";

marked.setOptions({ breaks: true, gfm: true });

/** Render markdown to sanitized HTML, safe to pass to `{@html ...}`. */
export function renderMarkdown(src: string): string {
  const html = marked.parse(src ?? "", { async: false }) as string;
  return DOMPurify.sanitize(html);
}
