// Exclude comments so examples mentioned in comments don't change the mode.
export function isLatexDocument(text: string): boolean {
  const source = text.replace(/(?<!\\)%[^\r\n]*/g, '')
  return /\\(?:documentclass\b|(?:title|author|section|subsection|chapter|paragraph)\s*\{|begin\s*\{(?:document|abstract|itemize|enumerate|tabular|figure|table)\})/.test(source)
}
