/**
 * Reflow a YAML literal block scalar into paragraphs.
 *
 * Workflow descriptions are written as `description: |`, which preserves **every**
 * newline — including the soft wraps the author used to keep the YAML narrow.
 * Rendered with `whitespace-pre-line` those soft wraps become hard breaks at
 * whatever column the YAML happened to wrap at, so the text arrives pre-broken
 * into ragged short lines that ignore the width it is being shown in.
 *
 * This collapses the soft wraps back into spaces while keeping blank lines as
 * real paragraph breaks — the treatment YAML's folded (`>`) scalar would have
 * given, applied at render time so the source stays readable as authored.
 */
export function reflowParagraphs(text: string): string {
  return text
    .split(/\n[ \t]*\n+/) // a blank line separates paragraphs
    .map((paragraph) => paragraph.replace(/\s*\n\s*/g, " ").trim())
    .filter(Boolean)
    .join("\n\n");
}
