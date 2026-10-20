import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { X } from 'lucide-react'
import { fieldLabel, MAX_STRUCTURED_CHARACTERS, preferenceKey, structuredValue, type FieldPath } from '../contentPreview'
import { ContentPreview } from './ContentPreview'
import { JsonFieldTree } from './JsonFieldTree'
import { connectedUrl } from '../api'
import type { RowsResponse, TableData } from '../types'

type MediaKind = 'image' | 'audio' | 'video' | 'blob'

const mediaKind = (mime: string): MediaKind => mime.startsWith('image/')
  ? 'image'
  : mime.startsWith('audio/')
    ? 'audio'
    : mime.startsWith('video/')
      ? 'video'
      : 'blob'

function MediaPreview({
  source,
  alt,
}: {
  source: string
  alt: string
}) {
  const [kind, setKind] = useState<MediaKind>()
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    setKind(undefined)
    setFailed(false)
    const controller = new AbortController()
    fetch(source, {
      method: 'HEAD',
      signal: controller.signal,
    })
      .then((response) => {
        if (!response.ok) throw new Error(`media request failed: ${response.status}`)
        setKind(mediaKind(response.headers.get('Content-Type') ?? ''))
      })
      .catch(() => {
        if (!controller.signal.aborted) setFailed(true)
      })
    return () => controller.abort()
  }, [source])

  let content: ReactNode = <span className="media-loading">Loading</span>
  if (failed) {
    content = <span className="media-loading media-error">Failed to load</span>
  } else if (kind === 'image') {
    content = <img className="media-image" src={source} alt={alt} />
  } else if (kind === 'audio') {
    content = <audio className="media-audio" src={source} controls preload="metadata" />
  } else if (kind === 'video') {
    content = <video className="media-video" src={source} controls preload="metadata" />
  } else if (kind === 'blob') {
    content = <a className="blob-link" href={source} target="_blank" rel="noopener noreferrer">open blob</a>
  }
  return (
    <div className={`media-slot media-slot-${kind ?? 'blob'}`}>
      {content}
    </div>
  )
}

function MediaValue({
  column,
  indexPaths,
  rowAddress,
  connectionId,
}: {
  column: RowsResponse['media_columns'][number]
  indexPaths: number[][]
  rowAddress: unknown
  connectionId: string
}) {
  const container = useRef<HTMLDivElement>(null)
  const [visible, setVisible] = useState(false)

  useEffect(() => {
    const element = container.current
    if (!element || visible) return
    if (!('IntersectionObserver' in window)) {
      setVisible(true)
      return
    }
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setVisible(true)
          observer.disconnect()
        }
      },
      { root: element.closest('.table-scroll'), rootMargin: '200px' },
    )
    observer.observe(element)
    return () => observer.disconnect()
  }, [visible])

  if (rowAddress == null || indexPaths.length === 0) return <span>—</span>
  return (
    <div ref={container} className="media-array">
      {!visible
        ? <div className="media-slot media-slot-blob"><span className="media-loading">Blob</span></div>
        : indexPaths.map((indexPath) => {
          const indexQuery = indexPath.length === 0 ? '' : `&index=${indexPath.join(',')}`
          const source = connectedUrl(
            `/api/media/${column.source_field_id}/${rowAddress}`,
            connectionId,
          ) + indexQuery
          const itemLabel = indexPath.length === 0
            ? column.name
            : `${column.name} ${indexPath.map((index) => index + 1).join('.')}`
          return (
            <MediaPreview
              key={indexPath.join(',')}
              source={source}
              alt={`${itemLabel} preview`}
            />
          )
        })}
    </div>
  )
}

const displayValue = (value: unknown) => {
  if (typeof value === 'object' && value != null) return JSON.stringify(value)
  return String(value)
}

const formattedJson = (value: unknown) => {
  if (typeof value === 'object' && value != null) return JSON.stringify(value, null, 2)
  if (typeof value !== 'string' || value.length > MAX_STRUCTURED_CHARACTERS) return undefined
  const trimmed = value.trim()
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) return undefined
  try {
    const parsed = JSON.parse(trimmed)
    return typeof parsed === 'object' && parsed != null
      ? JSON.stringify(parsed, null, 2)
      : undefined
  } catch {
    return undefined
  }
}

const hexPattern = /^(?:[0-9a-f]{2})+$/i
const hasControlCharacters = (text: string) => {
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index)
    if ((code < 32 && code !== 9 && code !== 10 && code !== 13) || code === 127) return true
  }
  return false
}

// arrow-json serializes Binary/LargeBinary columns as lowercase hex strings.
const decodeHexText = (value: unknown) => {
  if (typeof value !== 'string' || !hexPattern.test(value)) return undefined
  const bytes = new Uint8Array(value.length / 2)
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = parseInt(value.slice(index * 2, index * 2 + 2), 16)
  }
  try {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
    return hasControlCharacters(text) ? undefined : text
  } catch {
    return undefined
  }
}

type InspectedValue = {
  column: string
  value: unknown
}

function ScalarValue({
  value,
  column,
  onInspect,
}: {
  value: unknown
  column: string
  onInspect: (value: InspectedValue) => void
}) {
  let content
  if (value == null) content = <span className="null-value">null</span>
  else if (typeof value === 'boolean') content = <span className={value ? 'bool-true' : 'bool-false'}>{String(value)}</span>
  else if (typeof value === 'object') content = <code>{displayValue(value)}</code>
  else content = <span>{String(value)}</span>

  if (value == null) return <div className="scalar-value">{content}</div>
  return (
    <button
      type="button"
      className="scalar-value scalar-value-button"
      onClick={() => onInspect({ column, value })}
      title="View full value"
    >
      {content}
    </button>
  )
}

function ValueInspector({ inspected, datasetKey, onClose }: {
  inspected: InspectedValue
  datasetKey: string
  onClose: () => void
}) {
  const [wrap, setWrap] = useState(true)
  const [pretty, setPretty] = useState(true)
  const decoded = useMemo(() => decodeHexText(inspected.value), [inspected.value])
  const [selected, setSelected] = useState<{ path: FieldPath; value: unknown }>()
  const root = decoded ?? inspected.value
  const structured = useMemo(() => structuredValue(root), [root])
  const selectedValue = selected?.value ?? root
  const label = fieldLabel(inspected.column, selected?.path ?? [])
  const preference = preferenceKey(datasetKey, inspected.column, selected?.path ?? [])
  const [showFields, setShowFields] = useState(true)

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', closeOnEscape)
    return () => window.removeEventListener('keydown', closeOnEscape)
  }, [onClose])

  return (
    <>
      <button className="value-inspector-backdrop" type="button" aria-label="Close value inspector" onClick={onClose} />
      <aside className="value-inspector" role="dialog" aria-modal="true" aria-labelledby="value-inspector-title">
        <header className="value-inspector-header">
          <div><span className="eyebrow">Full cell value</span><h2 id="value-inspector-title">{inspected.column}</h2></div>
          <button className="icon-button" type="button" aria-label="Close value inspector" onClick={onClose}><X size={17} /></button>
        </header>
        {structured && (
          <section className="inspector-fields">
            <button className="inspector-fields-toggle" type="button" aria-expanded={showFields} onClick={() => setShowFields(!showFields)}>
              {showFields ? '▾' : '▸'} JSON fields · select a value to preview
            </button>
            {showFields && <div className="inspector-field-tree"><JsonFieldTree value={root} label={inspected.column} selectedPath={selected?.path ?? []} onSelect={(path, value) => setSelected({ path, value })} /></div>}
          </section>
        )}
        <div className="value-inspector-selection" title={label}>{label}</div>
        <ContentPreview
          key={`${preference}:${typeof selectedValue}`}
          value={selectedValue}
          rawValue={selected ? selected.value : inspected.value}
          decoded={selected ? undefined : decoded}
          label={label}
          preference={preference}
          wrap={wrap}
          pretty={pretty}
          onWrap={() => setWrap(!wrap)}
          onPretty={() => setPretty(!pretty)}
          formattedJson={formattedJson}
        />
      </aside>
    </>
  )
}

export function RowsTable({
  data,
  connectionId,
  datasetKey = connectionId,
  footer,
}: {
  data: TableData
  connectionId: string
  datasetKey?: string
  footer?: ReactNode
}) {
  const [inspected, setInspected] = useState<InspectedValue>()
  const [inspectionId, setInspectionId] = useState(0)
  const inspect = (value: InspectedValue) => { setInspected(value); setInspectionId((id) => id + 1) }
  const scalarColumns = data.columns.filter((column) => column !== '_rowaddr')
  return (
    <div className="table-scroll">
      <table>
        <thead><tr>
          {scalarColumns.map((column) => <th key={column}>{column}</th>)}
          {data.media_columns.map((column) => <th key={column.name}>{column.name}</th>)}
        </tr></thead>
        <tbody>
          {data.rows.map((row, index) => (
            <tr key={String(row.values._rowaddr ?? index)}>
              {scalarColumns.map((column) => (
                <td key={column}>
                  <ScalarValue value={row.values[column]} column={column} onInspect={inspect} />
                </td>
              ))}
              {data.media_columns.map((column) => (
                <td key={column.name}>
                  <MediaValue
                    column={column}
                    indexPaths={row.media[column.name] ?? []}
                    rowAddress={row.values._rowaddr}
                    connectionId={connectionId}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {footer}
      {inspected && <ValueInspector key={inspectionId} inspected={inspected} datasetKey={datasetKey} onClose={() => setInspected(undefined)} />}
    </div>
  )
}
