import { useEffect, useRef, useState, type ReactNode } from 'react'
import { connectedUrl } from '../api'
import type { RowsResponse, TableData } from '../types'

function MediaValue({
  column,
  row,
  connectionId,
}: {
  column: RowsResponse['media_columns'][number]
  row: Record<string, unknown>
  connectionId: string
}) {
  const container = useRef<HTMLDivElement>(null)
  const [nearViewport, setNearViewport] = useState(false)
  const rowAddress = row._rowaddr
  const mime = column.mime_column ? String(row[column.mime_column] ?? '') : ''
  const kind = mime.startsWith('image/')
    ? 'image'
    : mime.startsWith('audio/')
      ? 'audio'
      : mime.startsWith('video/')
        ? 'video'
        : 'blob'

  useEffect(() => {
    const element = container.current
    if (!element || nearViewport) return
    if (!('IntersectionObserver' in window)) {
      setNearViewport(true)
      return
    }
    const scrollContainer = element.closest('.table-scroll')
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setNearViewport(true)
          observer.disconnect()
        }
      },
      { root: scrollContainer, rootMargin: '160px 240px', threshold: 0.01 },
    )
    observer.observe(element)
    return () => observer.disconnect()
  }, [nearViewport])

  if (rowAddress == null) return <span>—</span>
  const source = connectedUrl(
    `/api/media/${encodeURIComponent(column.name)}/${rowAddress}?mime=${encodeURIComponent(mime)}`,
    connectionId,
  )
  let content
  if (!nearViewport) {
    content = <span className="media-lazy-label">Blob</span>
  } else if (kind === 'image') {
    content = <img className="media-image" src={source} alt={`${column.name} preview`} />
  } else if (kind === 'audio') {
    content = <audio className="media-audio" src={source} controls preload="metadata" />
  } else if (kind === 'video') {
    content = <video className="media-video" src={source} controls preload="metadata" />
  } else {
    content = <a className="blob-link" href={source} target="_blank">open blob</a>
  }
  return (
    <div
      ref={container}
      className={`media-lazy-slot media-lazy-${kind}`}
      data-media-state={nearViewport ? 'ready' : 'deferred'}
    >
      {content}
    </div>
  )
}

function ScalarValue({ value }: { value: unknown }) {
  if (value == null) return <span className="null-value">null</span>
  if (typeof value === 'boolean') return <span className={value ? 'bool-true' : 'bool-false'}>{String(value)}</span>
  if (typeof value === 'object') return <code>{JSON.stringify(value)}</code>
  return <span>{String(value)}</span>
}

export function RowsTable({
  data,
  connectionId,
  footer,
}: {
  data: TableData
  connectionId: string
  footer?: ReactNode
}) {
  return (
    <div className="table-scroll">
      <table>
        <thead><tr>
          {data.columns.filter((column) => column !== '_rowaddr').map((column) => <th key={column}>{column}</th>)}
          {data.media_columns.map((column) => <th key={column.name}>{column.name}</th>)}
        </tr></thead>
        <tbody>
          {data.rows.map((row, index) => (
            <tr key={String(row._rowaddr ?? index)}>
              {data.columns.filter((column) => column !== '_rowaddr').map((column) => (
                <td key={column}><ScalarValue value={row[column]} /></td>
              ))}
              {data.media_columns.map((column) => (
                <td key={column.name}><MediaValue column={column} row={row} connectionId={connectionId} /></td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {footer}
    </div>
  )
}
