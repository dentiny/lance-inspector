export type ContentView = 'raw' | 'text' | 'markdown' | 'html'
export type FieldPath = (string | number)[]
export const MAX_STRUCTURED_CHARACTERS = 2 * 1024 * 1024

// Parse only inspected values. Large payloads remain available as source text.
export function structuredValue(value: unknown): object | undefined {
  if (typeof value === 'object' && value !== null) return value
  if (typeof value !== 'string' || value.length > MAX_STRUCTURED_CHARACTERS) return undefined
  const text = value.trim()
  if (!text.startsWith('{') && !text.startsWith('[')) return undefined
  try {
    const parsed: unknown = JSON.parse(text)
    return typeof parsed === 'object' && parsed !== null ? parsed : undefined
  } catch {
    // A JSONL cell can contain several complete records. Never parse a partial record.
    const lines = text.split(/\r?\n/).filter((line) => line.trim())
    if (lines.length < 2) return undefined
    try {
      const records: unknown[] = lines.map((line) => JSON.parse(line))
      return records.every((record) => typeof record === 'object' && record !== null) ? records : undefined
    } catch {
      return undefined
    }
  }
}

export function fieldLabel(column: string, path: FieldPath): string {
  return column + path.map((key) => typeof key === 'number'
    ? `[${key}]`
    : /^[a-zA-Z_$][\w$]*$/.test(key) ? `.${key}` : `[${JSON.stringify(key)}]`).join('')
}

// Array indices share preferences, while numeric object keys remain distinct.
export function preferenceKey(dataset: string, column: string, path: FieldPath): string {
  return `lance-content-view:${JSON.stringify([dataset, column, path.map((key) => typeof key === 'number' ? null : key)])}`
}

export function sourceText(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value, null, 2) ?? String(value)
}
