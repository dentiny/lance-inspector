import { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowRight, CircleAlert, RefreshCw } from 'lucide-react'
import { connectedUrl, HttpError, requireOk } from '../api'
import type { DatasetInfo, SqlCursorResponse, SqlPageResponse, TableData } from '../types'
import { RowsTable } from './RowsTable'

const DEFAULT_SQL = 'SELECT * FROM dataset'
const MAX_SQL_RESULT_ROWS = 10_000

const cancelSqlCursor = (cursorId: string | undefined, connectionId: string) => {
  if (!cursorId) return
  void fetch(connectedUrl(`/api/sql/${encodeURIComponent(cursorId)}/cancel`, connectionId), {
    method: 'POST',
  }).catch(() => undefined)
}

function SqlQueryView({ connectionId }: { connectionId: string }) {
  const [sql, setSql] = useState(DEFAULT_SQL)
  const [data, setData] = useState<TableData>()
  const [error, setError] = useState('')
  const [streaming, setStreaming] = useState(false)
  const [truncated, setTruncated] = useState(false)
  const [hasMore, setHasMore] = useState(true)
  const [retryRequired, setRetryRequired] = useState(false)
  const [rerunRequired, setRerunRequired] = useState(false)
  const request = useRef<AbortController | undefined>(undefined)
  const cursor = useRef<string | undefined>(undefined)
  const appliedSql = useRef(DEFAULT_SQL)
  const nextSequence = useRef(0)
  const generation = useRef(0)
  const loading = useRef(false)
  const loadMoreSentinel = useRef<HTMLDivElement>(null)

  const loadPage = useCallback(async (cursorId: string, sequence: number, replace: boolean, run: number) => {
    if (loading.current || run !== generation.current) return
    loading.current = true
    const controller = new AbortController()
    request.current = controller
    setError('')
    setStreaming(true)
    try {
      const response = await fetch(connectedUrl(
        `/api/sql/${encodeURIComponent(cursorId)}/page?sequence=${sequence}`,
        connectionId,
      ), { signal: controller.signal })
      await requireOk(response)
      const page = await response.json() as SqlPageResponse
      if (run !== generation.current || cursor.current !== cursorId) return
      setData((current) => replace || !current
        ? { ...(current ?? { columns: [], media_columns: [] }), rows: page.rows }
        : { ...current, rows: [...current.rows, ...page.rows] })
      nextSequence.current = page.sequence + 1
      setHasMore(!page.done)
      setTruncated(page.truncated)
      setRetryRequired(false)
      setRerunRequired(false)
    } catch (reason) {
      if (!controller.signal.aborted && run === generation.current) {
        const rerun = reason instanceof HttpError && (reason.status === 404 || reason.status === 422)
        setError(reason instanceof Error ? reason.message : String(reason))
        setRetryRequired(!rerun)
        setRerunRequired(rerun)
      }
    } finally {
      if (request.current === controller && run === generation.current) {
        loading.current = false
        setStreaming(false)
      }
    }
  }, [connectionId])

  const execute = useCallback(async (statement: string) => {
    const run = generation.current + 1
    generation.current = run
    appliedSql.current = statement
    request.current?.abort()
    cancelSqlCursor(cursor.current, connectionId)
    cursor.current = undefined
    loading.current = false
    setData(undefined)
    setError('')
    setStreaming(true)
    setTruncated(false)
    setHasMore(true)
    setRetryRequired(false)
    setRerunRequired(false)
    const controller = new AbortController()
    request.current = controller
    loading.current = true
    try {
      const response = await fetch(connectedUrl('/api/sql/start', connectionId), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sql: statement }),
        signal: controller.signal,
      })
      await requireOk(response)
      const started = await response.json() as SqlCursorResponse
      if (run !== generation.current) {
        cancelSqlCursor(started.cursor_id, connectionId)
        return
      }
      cursor.current = started.cursor_id
      nextSequence.current = 0
      setData({ columns: started.columns, media_columns: started.media_columns, rows: [] })
      loading.current = false
      await loadPage(started.cursor_id, 0, true, run)
    } catch (reason) {
      if (!controller.signal.aborted && run === generation.current) {
        setError(reason instanceof Error ? reason.message : String(reason))
      }
    } finally {
      if (request.current === controller && run === generation.current) {
        loading.current = false
        setStreaming(false)
      }
    }
  }, [connectionId, loadPage])

  const loadMore = useCallback((retry = false) => {
    if (!streaming && hasMore && data && cursor.current && !rerunRequired && (!retryRequired || retry)) {
      void loadPage(cursor.current, nextSequence.current, false, generation.current)
    }
  }, [data, hasMore, loadPage, rerunRequired, retryRequired, streaming])

  useEffect(() => {
    void execute(DEFAULT_SQL)
    return () => {
      generation.current += 1
      request.current?.abort()
      cancelSqlCursor(cursor.current, connectionId)
    }
  }, [connectionId, execute])

  useEffect(() => {
    const element = loadMoreSentinel.current
    if (!element || !hasMore) return
    const root = element.closest('.table-scroll')
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) loadMore()
      },
      { root, rootMargin: '300px 0px', threshold: 0.01 },
    )
    observer.observe(element)
    return () => observer.disconnect()
  }, [data?.rows.length, hasMore, loadMore])

  const submit = (event: React.FormEvent) => {
    event.preventDefault()
    const statement = sql.trim()
    if (statement) void execute(statement)
  }

  return (
    <>
      <section className="panel sql-panel">
        <div className="panel-title">
          <div><span className="eyebrow">DataFusion · read only</span><h2>SQL query</h2></div>
          <code>table: dataset</code>
        </div>
        <form onSubmit={submit}>
          <textarea
            value={sql}
            onChange={(event) => setSql(event.target.value)}
            onKeyDown={(event) => {
              if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
                event.preventDefault()
                event.currentTarget.form?.requestSubmit()
              }
            }}
            aria-label="SQL query"
            spellCheck={false}
          />
          <div className="sql-actions">
            <span>SELECT or WITH · ⌘/Ctrl + Enter</span>
            <button type="submit" disabled={streaming || !sql.trim()}>
              {streaming ? <RefreshCw className="spin" size={14} /> : <ArrowRight size={14} />}
              {streaming ? 'Streaming' : 'Apply'}
            </button>
          </div>
        </form>
      </section>
      <section className="panel data-panel sql-results">
        <div className="panel-title">
          <div><span className="eyebrow">Incremental result stream</span><h2>Query results</h2></div>
          {data && (
            <span className="count-badge">
              {data.rows.length.toLocaleString()} rows
              {streaming ? ' · streaming' : ''}
              {truncated ? ` · capped at ${MAX_SQL_RESULT_ROWS.toLocaleString()}` : ''}
            </span>
          )}
        </div>
        {error && (
          <div className="error-state">
            <CircleAlert />{error}
            {rerunRequired
              ? <button onClick={() => void execute(appliedSql.current)}>Rerun query</button>
              : cursor.current && <button onClick={() => loadMore(true)}>Retry page</button>}
          </div>
        )}
        {!data && streaming && <div className="loading-state"><RefreshCw className="spin" />Planning SQL query…</div>}
        {data && data.rows.length === 0 && streaming && <div className="loading-state"><RefreshCw className="spin" />Waiting for rows…</div>}
        {data && data.rows.length > 0 && (
          <RowsTable
            data={data}
            connectionId={connectionId}
            footer={(
              <div ref={loadMoreSentinel} className="loading-state">
                {streaming
                  ? <><RefreshCw className="spin" />Loading more rows…</>
                  : hasMore
                    ? rerunRequired ? 'Rerun query required' : retryRequired ? 'Retry required' : 'Scroll to load more rows'
                    : truncated
                      ? 'Result capped at 10,000 rows'
                      : 'End of results'}
              </div>
            )}
          />
        )}
        {data && data.rows.length === 0 && !streaming && !error && <div className="empty-state">Query returned no rows.</div>}
      </section>
    </>
  )
}

export function DatasetQueryView({
  info,
  mode,
  connectionId,
}: {
  info: DatasetInfo
  mode: 'infra' | 'user'
  connectionId: string
}) {
  return (
    <div className="page wide-page user-data-page">
      <div className="page-heading">
        <div>
          <span className="eyebrow">{mode === 'infra' ? 'Infra mode · read-only SQL' : 'User mode · selected snapshot'}</span>
          <h1>{mode === 'infra' ? 'Query dataset' : 'Dataset data'}</h1>
          <p>{info.uri}</p>
        </div>
        <span className="count-badge">{info.rows.toLocaleString()} rows · {info.branch} v{info.version}</span>
      </div>
      <SqlQueryView connectionId={connectionId} />
    </div>
  )
}
