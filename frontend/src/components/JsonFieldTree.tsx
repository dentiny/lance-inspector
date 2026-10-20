import { useMemo, useState } from 'react'
import { sourceText, structuredValue, type FieldPath } from '../contentPreview'

const PAGE_SIZE = 50
const MAX_DEPTH = 32

export function JsonFieldTree({ value, label, path = [], selectedPath, onSelect }: {
  value: unknown
  label: string
  path?: FieldPath
  selectedPath: FieldPath
  onSelect: (path: FieldPath, value: unknown) => void
}) {
  const [expanded, setExpanded] = useState(path.length === 0)
  const [limit, setLimit] = useState(PAGE_SIZE)
  const structured = useMemo(() => structuredValue(value), [value])
  const selected = JSON.stringify(path) === JSON.stringify(selectedPath)
  const selectable = value !== null && value !== undefined
  const select = <button type="button" className={`json-field-value ${selected ? 'selected' : ''}`} disabled={!selectable} onClick={() => onSelect(path, value)} title={label}>
    <strong>{label}</strong><span>{structured ? `${Array.isArray(structured) ? 'Array' : 'Object'}${typeof value === 'string' ? ' · JSON text' : ''}` : sourceText(value).slice(0, 100)}</span>
  </button>
  if (!structured || path.length >= MAX_DEPTH) return <div className="json-field-leaf">{select}</div>
  // Children mount only when expanded; large collections are revealed in small pages.
  const entries = expanded ? Object.entries(structured) : []
  return (
    <div className="json-field-node">
      <div className="json-field-row"><button className="json-field-expand" type="button" aria-label={`${expanded ? 'Collapse' : 'Expand'} ${label}`} aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>{expanded ? '▾' : '▸'}</button>{select}</div>
      {expanded && <div className="json-field-children">
        {entries.slice(0, limit).map(([key, child]) => {
          const part = Array.isArray(structured) ? Number(key) : key
          return <JsonFieldTree key={key} value={child} label={Array.isArray(structured) ? `[${key}]` : key} path={[...path, part]} selectedPath={selectedPath} onSelect={onSelect} />
        })}
        {entries.length > limit && <button className="json-field-more" type="button" onClick={() => setLimit(limit + PAGE_SIZE)}>Show {Math.min(PAGE_SIZE, entries.length - limit)} more ({entries.length - limit} remaining)</button>}
      </div>}
    </div>
  )
}
