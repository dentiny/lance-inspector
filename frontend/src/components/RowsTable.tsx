import { useEffect, useRef, useState, type ReactNode } from 'react'
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
                <td key={column}><ScalarValue value={row.values[column]} /></td>
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
    </div>
  )
}
