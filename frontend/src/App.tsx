import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Activity,
  ArrowRight,
  Braces,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  Database,
  File,
  FileArchive,
  FileJson,
  FileX2,
  Folder,
  FolderOpen,
  GitBranch,
  HardDrive,
  History,
  Image as ImageIcon,
  Layers3,
  RefreshCw,
  Rows3,
  Search,
  Tag,
} from 'lucide-react'

type SchemaField = {
  name: string
  data_type: string
  nullable: boolean
  media: boolean
  metadata: Record<string, string>
}

type Deletion = {
  path: string
  file_type: string
  read_version: number
  id: number
  count: number
  offsets: number[]
  offsets_truncated: boolean
}

type Fragment = {
  id: number
  physical_rows: number | null
  visible_rows: number | null
  files: {
    path: string
    fields: number[]
    column_indices: number[]
    format: string
    size_bytes: number | null
    base_id: number | null
  }[]
  deletion: Deletion | null
}

type DatasetInfo = {
  uri: string
  reference: string
  version: number
  branch: string
  rows: number
  schema: SchemaField[]
  manifest: Record<string, unknown>
  fragments: Fragment[]
  branches: { name: string; parent_branch: string | null; parent_version: number }[]
}

type ConnectedDataset = {
  connection_id: string
  dataset: DatasetInfo
}

type ReferenceCatalog = {
  uri: string
  branches: {
    name: string
    parent_branch: string | null
    parent_version: number | null
    versions: {
      version: number
      timestamp: string
      total_rows: number | null
      tags: string[]
    }[]
  }[]
  tags: { name: string; branch: string; version: number }[]
}

type FileEntry = {
  path: string
  size: number
  kind: 'manifest' | 'data' | 'deletion' | 'index' | 'transaction' | 'file'
  modified: string
}

type TreeNode = {
  name: string
  path: string
  children: TreeNode[]
  file?: FileEntry
}

type RowsResponse = {
  offset: number
  limit: number
  total: number
  columns: string[]
  media_columns: { name: string; mime_column: string | null }[]
  rows: Record<string, unknown>[]
}

type TableData = Pick<RowsResponse, 'columns' | 'media_columns' | 'rows'>

type SqlCursorResponse = {
  cursor_id: string
  columns: string[]
  media_columns: RowsResponse['media_columns']
}

type SqlPageResponse = {
  sequence: number
  rows: Record<string, unknown>[]
  done: boolean
  truncated: boolean
}

type TransactionInfo = {
  path: string
  read_version: number
  uuid: string
  tag: string | null
  properties: Record<string, string>
  operation_type: string
  operation: Record<string, unknown>
}

type Selection = { type: 'overview' } | { type: 'sql' } | { type: 'file'; file: FileEntry }

const connectedUrl = (path: string, connectionId: string) => {
  const separator = path.includes('?') ? '&' : '?'
  return `${path}${separator}connection_id=${encodeURIComponent(connectionId)}`
}

class HttpError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = 'HttpError'
    this.status = status
  }
}

async function requireOk(response: Response) {
  if (response.ok) return response
  const body = await response.json().catch(() => ({ error: response.statusText }))
  if (response.status === 410) {
    window.dispatchEvent(new Event('lance-connection-expired'))
  }
  throw new HttpError(body.error ?? response.statusText, response.status)
}

const formatBytes = (bytes: number | null) => {
  if (bytes == null) return 'unknown'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = bytes / 1024
  let unit = units[0]
  for (let index = 1; value >= 1024 && index < units.length; index += 1) {
    value /= 1024
    unit = units[index]
  }
  return `${value.toFixed(value >= 10 ? 1 : 2)} ${unit}`
}

function buildTree(files: FileEntry[]): TreeNode[] {
  const root: TreeNode = { name: '', path: '', children: [] }
  for (const file of files) {
    let current = root
    const parts = file.path.split('/')
    parts.forEach((part, index) => {
      const path = parts.slice(0, index + 1).join('/')
      let node = current.children.find((child) => child.name === part)
      if (!node) {
        node = { name: part, path, children: [] }
        current.children.push(node)
      }
      if (index === parts.length - 1) node.file = file
      current = node
    })
  }
  const sort = (nodes: TreeNode[]) => {
    nodes.sort((left, right) => {
      if (!!left.file !== !!right.file) return left.file ? 1 : -1
      return left.name.localeCompare(right.name)
    })
    nodes.forEach((node) => sort(node.children))
  }
  sort(root.children)
  return root.children
}

function fileIcon(kind: FileEntry['kind']) {
  if (kind === 'manifest') return <FileJson size={15} />
  if (kind === 'data') return <Rows3 size={15} />
  if (kind === 'deletion') return <FileX2 size={15} />
  if (kind === 'index') return <Search size={15} />
  if (kind === 'transaction') return <FileArchive size={15} />
  return <File size={15} />
}

function TreeItem({
  node,
  selected,
  onSelect,
}: {
  node: TreeNode
  selected?: string
  onSelect: (file: FileEntry) => void
}) {
  const [open, setOpen] = useState(true)
  const directory = !node.file
  return (
    <div className="tree-node">
      <button
        className={`tree-row ${selected === node.path ? 'selected' : ''}`}
        onClick={() => (directory ? setOpen(!open) : onSelect(node.file!))}
        title={node.path}
      >
        <span className="tree-chevron">
          {directory ? open ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : null}
        </span>
        {directory ? open ? <FolderOpen size={15} /> : <Folder size={15} /> : fileIcon(node.file!.kind)}
        <span className="tree-name">{node.name}</span>
        {node.file && <span className="tree-size">{formatBytes(node.file.size)}</span>}
      </button>
      {directory && open && (
        <div className="tree-children">
          {node.children.map((child) => (
            <TreeItem key={child.path} node={child} selected={selected} onSelect={onSelect} />
          ))}
        </div>
      )}
    </div>
  )
}

function StatCard({
  icon,
  label,
  value,
  detail,
}: {
  icon: React.ReactNode
  label: string
  value: string | number
  detail: string
}) {
  return (
    <div className="stat-card">
      <div className="stat-icon">{icon}</div>
      <div>
        <span>{label}</span>
        <strong>{value}</strong>
        <small>{detail}</small>
      </div>
    </div>
  )
}

function DeletionGrid({ fragment }: { fragment: Fragment }) {
  const physical = fragment.physical_rows ?? 0
  const deleted = new Set(fragment.deletion?.offsets ?? [])
  if (!fragment.deletion) return <div className="empty-inline">No deletion vector for this fragment.</div>
  return (
    <section className="panel deletion-panel">
      <div className="panel-title">
        <div>
          <span className="eyebrow">Deletion vector</span>
          <h3>{fragment.deletion.path}</h3>
        </div>
        <span className="danger-badge">{fragment.deletion.count} deleted</span>
      </div>
      <div className="deletion-summary">
        <div>
          <strong>{fragment.deletion.count}</strong>
          <span>deleted rows</span>
        </div>
        <div>
          <strong>{physical ? ((fragment.deletion.count / physical) * 100).toFixed(2) : '0'}%</strong>
          <span>of physical rows</span>
        </div>
        <div>
          <strong>{fragment.deletion.file_type}</strong>
          <span>encoding</span>
        </div>
      </div>
      <div className="row-grid" aria-label="Physical rows; red cells are deleted">
        {Array.from({ length: Math.min(physical, 500) }, (_, offset) => (
          <span
            key={offset}
            className={deleted.has(offset) ? 'deleted' : ''}
            title={`Physical row ${offset}${deleted.has(offset) ? ' — deleted' : ''}`}
          >
            {offset}
          </span>
        ))}
      </div>
      <div className="legend">
        <span><i className="live-swatch" /> live physical row</span>
        <span><i className="deleted-swatch" /> deleted physical row</span>
      </div>
      <div className="offset-list">
        <span>Deleted offsets</span>
        {fragment.deletion.offsets.map((offset) => <code key={offset}>{offset}</code>)}
        {fragment.deletion.offsets_truncated && <em>showing first 2,000</em>}
      </div>
    </section>
  )
}

function Overview({ info }: { info: DatasetInfo }) {
  const deletionCount = info.fragments.reduce((total, fragment) => total + (fragment.deletion?.count ?? 0), 0)
  const dataSize = info.fragments.flatMap((fragment) => fragment.files)
    .reduce((total, file) => total + (file.size_bytes ?? 0), 0)
  return (
    <div className="page">
      <div className="page-heading">
        <div>
          <span className="eyebrow">Dataset overview</span>
          <h1>Storage anatomy</h1>
          <p>Version, schema, fragments, and physical layout of the mounted Lance dataset.</p>
        </div>
        <span className="read-only"><Activity size={14} /> read only</span>
      </div>
      <div className="stats-grid">
        <StatCard icon={<Rows3 />} label="Visible rows" value={info.rows.toLocaleString()} detail={`${deletionCount} physically deleted`} />
        <StatCard icon={<Layers3 />} label="Fragments" value={info.fragments.length} detail={`${info.schema.length} top-level fields`} />
        <StatCard icon={<HardDrive />} label="Data size" value={formatBytes(dataSize)} detail="manifest-reported" />
        <StatCard icon={<GitBranch />} label="Version" value={`v${info.version}`} detail={`${info.branch} · ${info.branches.length} child branch${info.branches.length === 1 ? '' : 'es'}`} />
      </div>
      <section className="panel">
        <div className="panel-title">
          <div><span className="eyebrow">Logical model</span><h2>Schema</h2></div>
          <span className="count-badge">{info.schema.length} fields</span>
        </div>
        <div className="schema-list">
          {info.schema.map((field, index) => (
            <div className="schema-row" key={field.name}>
              <span className="field-index">{String(index + 1).padStart(2, '0')}</span>
              <strong>{field.name}</strong>
              <code>{field.data_type}</code>
              {field.media && <span className="media-badge"><ImageIcon size={12} /> media</span>}
              <span className={field.nullable ? 'nullable' : 'required'}>{field.nullable ? 'nullable' : 'required'}</span>
            </div>
          ))}
        </div>
      </section>
      <section className="panel">
        <div className="panel-title">
          <div><span className="eyebrow">Physical model</span><h2>Fragments</h2></div>
        </div>
        <div className="fragment-list">
          {info.fragments.map((fragment) => (
            <div className="fragment-card" key={fragment.id}>
              <div className="fragment-id"><Layers3 size={16} /><strong>Fragment {fragment.id}</strong></div>
              <span>{fragment.visible_rows ?? '?'} live / {fragment.physical_rows ?? '?'} physical rows</span>
              <span>{fragment.files.length} data file{fragment.files.length === 1 ? '' : 's'}</span>
              {fragment.deletion
                ? <span className="danger-badge">{fragment.deletion.count} deleted</span>
                : <span className="healthy-badge">no deletions</span>}
            </div>
          ))}
        </div>
      </section>
      {info.branches.length > 0 && (
        <section className="panel">
          <div className="panel-title"><div><span className="eyebrow">References</span><h2>Branches</h2></div></div>
          <div className="branch-list">
            <div className="branch-row"><GitBranch size={15} /><strong>main</strong><span>current · v{info.version}</span></div>
            {info.branches.map((branch) => (
              <div className="branch-row" key={branch.name}>
                <GitBranch size={15} /><strong>{branch.name}</strong>
                <span>from {branch.parent_branch ?? 'main'} · v{branch.parent_version}</span>
              </div>
            ))}
          </div>
        </section>
      )}
    </div>
  )
}

function ManifestView({ info, file }: { info: DatasetInfo; file: FileEntry }) {
  return (
    <div className="page">
      <div className="page-heading">
        <div><span className="eyebrow">Decoded protobuf</span><h1>Manifest</h1><p>{file.path}</p></div>
        <span className="count-badge">{formatBytes(file.size)}</span>
      </div>
      <section className="panel manifest-grid">
        {Object.entries(info.manifest).map(([key, value]) => (
          <div className="manifest-field" key={key}>
            <span>{key.replaceAll('_', ' ')}</span>
            {typeof value === 'object' && value !== null
              ? <pre>{JSON.stringify(value, null, 2)}</pre>
              : <strong>{value == null ? '—' : String(value)}</strong>}
          </div>
        ))}
      </section>
    </div>
  )
}

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
      {
        root: scrollContainer,
        rootMargin: '160px 240px',
        threshold: 0.01,
      },
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

function RowsTable({
  data,
  connectionId,
  footer,
}: {
  data: TableData
  connectionId: string
  footer?: React.ReactNode
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

function RowsPanel({
  refreshKey,
  connectionId,
  title = 'Row preview',
}: {
  refreshKey: string
  connectionId: string
  title?: string
}) {
  const [data, setData] = useState<RowsResponse>()
  const [offset, setOffset] = useState(0)
  const [error, setError] = useState('')
  useEffect(() => {
    const controller = new AbortController()
    setData(undefined)
    setError('')
    fetch(connectedUrl(`/api/rows?offset=${offset}&limit=20`, connectionId), {
      signal: controller.signal,
    })
      .then(async (response) => {
        await requireOk(response)
        return response.json() as Promise<RowsResponse>
      })
      .then(setData)
      .catch((reason: unknown) => {
        if (reason instanceof DOMException && reason.name === 'AbortError') return
        setError(reason instanceof Error ? reason.message : String(reason))
      })
    return () => controller.abort()
  }, [connectionId, offset, refreshKey])

  return (
    <section className="panel data-panel">
      <div className="panel-title">
        <div><span className="eyebrow">Live dataset scan</span><h2>{title}</h2></div>
        {data && <span className="count-badge">{offset + 1}–{Math.min(offset + data.rows.length, data.total)} of {data.total}</span>}
      </div>
      {error && <div className="error-state"><CircleAlert />{error}</div>}
      {!data && !error && <div className="loading-state"><RefreshCw className="spin" />Scanning Lance rows…</div>}
      {data && (
        <>
          <RowsTable data={data} connectionId={connectionId} />
          <div className="pagination">
            <button disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - 20))}>Previous</button>
            <span>20 rows per page</span>
            <button disabled={offset + 20 >= data.total} onClick={() => setOffset(offset + 20)}>Next</button>
          </div>
        </>
      )}
    </section>
  )
}

function DataView({ info, file, connectionId }: { info: DatasetInfo; file: FileEntry; connectionId: string }) {
  const fragment = info.fragments.find((item) => item.files.some((candidate) => candidate.path === file.path))
  return (
    <div className="page wide-page">
      <div className="page-heading">
        <div><span className="eyebrow">Data file · first 20 rows</span><h1>{file.path.split('/').at(-1)}</h1><p>{file.path}</p></div>
        <span className="count-badge">{formatBytes(file.size)}</span>
      </div>
      {fragment?.deletion && <DeletionGrid fragment={fragment} />}
      <RowsPanel key={file.path} refreshKey={file.path} connectionId={connectionId} />
    </div>
  )
}

const DEFAULT_SQL = 'SELECT * FROM dataset'
const MAX_SQL_RESULT_ROWS = 10_000

const cancelSqlCursor = (cursorId: string | undefined, connectionId: string) => {
  if (!cursorId) return
  void fetch(connectedUrl(`/api/sql/${encodeURIComponent(cursorId)}/cancel`, connectionId), {
    method: 'POST',
  }).catch(() => undefined)
}

function SqlQueryView({ snapshotKey, connectionId }: { snapshotKey: string; connectionId: string }) {
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
  }, [connectionId, execute, snapshotKey])

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

function DatasetQueryView({
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
      <SqlQueryView
        key={`${info.uri}:${info.branch}:${info.version}`}
        snapshotKey={`${info.uri}:${info.branch}:${info.version}`}
        connectionId={connectionId}
      />
    </div>
  )
}

function ScalarValue({ value }: { value: unknown }) {
  if (value == null) return <span className="null-value">null</span>
  if (typeof value === 'boolean') return <span className={value ? 'bool-true' : 'bool-false'}>{String(value)}</span>
  if (typeof value === 'object') return <code>{JSON.stringify(value)}</code>
  return <span>{String(value)}</span>
}

function DeletionFileView({ info, file }: { info: DatasetInfo; file: FileEntry }) {
  const fragment = info.fragments.find((item) => item.deletion?.path === file.path)
  return (
    <div className="page">
      <div className="page-heading">
        <div><span className="eyebrow">Physical tombstones</span><h1>Deletion vector</h1><p>{file.path}</p></div>
      </div>
      {fragment ? <DeletionGrid fragment={fragment} /> : <div className="empty-state">This deletion file is not referenced by the active manifest.</div>}
    </div>
  )
}

function TransactionFileView({ file, connectionId }: { file: FileEntry; connectionId: string }) {
  const [transaction, setTransaction] = useState<TransactionInfo>()
  const [error, setError] = useState('')
  useEffect(() => {
    const controller = new AbortController()
    setTransaction(undefined)
    setError('')
    fetch(connectedUrl(`/api/transaction?path=${encodeURIComponent(file.path)}`, connectionId), {
      signal: controller.signal,
    })
      .then(async (response) => {
        await requireOk(response)
        return response.json() as Promise<TransactionInfo>
      })
      .then(setTransaction)
      .catch((reason: unknown) => {
        if (reason instanceof DOMException && reason.name === 'AbortError') return
        setError(reason instanceof Error ? reason.message : String(reason))
      })
    return () => controller.abort()
  }, [connectionId, file.path])

  return (
    <div className="page">
      <div className="page-heading">
        <div><span className="eyebrow">Decoded protobuf</span><h1>Transaction</h1><p>{file.path}</p></div>
        {transaction && <span className="count-badge">{transaction.operation_type}</span>}
      </div>
      {error && <div className="error-state"><CircleAlert />{error}</div>}
      {!transaction && !error && <div className="loading-state"><RefreshCw className="spin" />Decoding transaction…</div>}
      {transaction && (
        <>
          <div className="stats-grid transaction-stats">
            <StatCard icon={<Activity />} label="Operation" value={transaction.operation_type} detail="protobuf oneof" />
            <StatCard icon={<GitBranch />} label="Read version" value={`v${transaction.read_version}`} detail="transaction base" />
            <StatCard icon={<Braces />} label="UUID" value={transaction.uuid.slice(0, 8)} detail={transaction.uuid} />
            <StatCard icon={<FileArchive />} label="File size" value={formatBytes(file.size)} detail={transaction.tag ?? 'no version tag'} />
          </div>
          <section className="panel">
            <div className="panel-title">
              <div><span className="eyebrow">Operation payload</span><h2>{transaction.operation_type}</h2></div>
            </div>
            <div className="transaction-fields">
              {Object.entries(transaction.operation).map(([key, value]) => (
                <div className="transaction-field" key={key}>
                  <span>{key.replaceAll('_', ' ')}</span>
                  {typeof value === 'object' && value !== null
                    ? <pre>{JSON.stringify(value, null, 2)}</pre>
                    : key === 'details'
                      ? <pre>{String(value)}</pre>
                      : <strong>{value == null || value === '' ? '—' : String(value)}</strong>}
                </div>
              ))}
            </div>
          </section>
          {Object.keys(transaction.properties).length > 0 && (
            <section className="panel">
              <div className="panel-title"><div><span className="eyebrow">Commit metadata</span><h2>Properties</h2></div></div>
              <div className="transaction-fields">
                {Object.entries(transaction.properties).map(([key, value]) => (
                  <div className="transaction-field" key={key}><span>{key}</span><strong>{value}</strong></div>
                ))}
              </div>
            </section>
          )}
        </>
      )}
    </div>
  )
}

function RawFileView({ file, connectionId }: { file: FileEntry; connectionId: string }) {
  const [preview, setPreview] = useState<{ content: string; format: string; truncated: boolean }>()
  const [error, setError] = useState('')
  useEffect(() => {
    const controller = new AbortController()
    setPreview(undefined)
    setError('')
    fetch(connectedUrl(`/api/file?path=${encodeURIComponent(file.path)}`, connectionId), {
      signal: controller.signal,
    })
      .then(async (response) => {
        await requireOk(response)
        return response.json()
      })
      .then(setPreview)
      .catch((reason: unknown) => {
        if (reason instanceof DOMException && reason.name === 'AbortError') return
        setError(reason instanceof Error ? reason.message : String(reason))
      })
    return () => controller.abort()
  }, [connectionId, file.path])
  return (
    <div className="page">
      <div className="page-heading">
        <div><span className="eyebrow">{file.kind} file</span><h1>{file.path.split('/').at(-1)}</h1><p>{file.path}</p></div>
        <span className="count-badge">{formatBytes(file.size)}</span>
      </div>
      {error && <div className="error-state"><CircleAlert />{error}</div>}
      {!preview && !error && <div className="loading-state"><RefreshCw className="spin" />Reading file…</div>}
      {preview && (
        <section className="panel raw-panel">
          <div className="panel-title"><h2>{preview.format === 'hex' ? 'Hex preview' : 'Text preview'}</h2>{preview.truncated && <span className="count-badge">first 64 KB</span>}</div>
          <pre>{preview.content}</pre>
        </section>
      )}
    </div>
  )
}

function DatasetConnector({
  currentUri,
  onDiscovered,
  onCancel,
}: {
  currentUri?: string
  onDiscovered: (catalog: ReferenceCatalog) => void
  onCancel?: () => void
}) {
  const [uri, setUri] = useState(currentUri ?? '')
  const [error, setError] = useState('')
  const [discovering, setDiscovering] = useState(false)

  const submit = (event: React.FormEvent) => {
    event.preventDefault()
    const location = uri.trim()
    if (!location) return
    setDiscovering(true)
    setError('')
    fetch('/api/dataset/references', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ uri: location }),
    })
      .then(async (response) => {
        const body = await response.json()
        if (!response.ok) throw new Error(body.error ?? response.statusText)
        return body as ReferenceCatalog
      })
      .then(onDiscovered)
      .catch((reason: Error) => setError(reason.message))
      .finally(() => setDiscovering(false))
  }

  return (
    <div className={onCancel ? 'connector-overlay' : 'connector-screen'}>
      <section className="connector-card">
        <div className="connector-brand"><span className="brand-mark"><Database /></span><strong>Lance Inspector</strong></div>
        <span className="eyebrow">Open a dataset</span>
        <h1>Inspect Lance storage</h1>
        <p>Enter a local dataset path or an S3 URI accessible to this server.</p>
        <form onSubmit={submit}>
          <div className="connector-fields">
            <div className="connector-control">
              <label htmlFor="dataset-location">Dataset location</label>
              <div className="connector-input">
                <HardDrive size={17} />
                <input
                  id="dataset-location"
                  value={uri}
                  onChange={(event) => setUri(event.target.value)}
                  placeholder="/data/example.lance or s3://bucket/path"
                  autoFocus
                  spellCheck={false}
                />
              </div>
            </div>
            <button className="connector-submit" type="submit" disabled={discovering || !uri.trim()}>
              {discovering ? <RefreshCw className="spin" size={16} /> : <ArrowRight size={16} />}
              {discovering ? 'Reading' : 'Continue'}
            </button>
          </div>
        </form>
        {error && <div className="connector-error"><CircleAlert size={15} />{error}</div>}
        <div className="connector-hints">
          <span><strong>References</strong> browse branches, versions, and tags next</span>
          <span><strong>Cloud</strong> S3 with server credentials</span>
          <span><strong>Safety</strong> read-only inspection</span>
        </div>
        {onCancel && <button className="connector-cancel" onClick={onCancel}>Cancel</button>}
      </section>
    </div>
  )
}

function LineageGraph({
  catalog,
  connecting,
  onSelect,
}: {
  catalog: ReferenceCatalog
  connecting: string
  onSelect: (reference: string) => void
}) {
  const versions = [...new Set(catalog.branches.flatMap((branch) => [
    ...branch.versions.map((version) => version.version),
    ...(branch.parent_version === null ? [] : [branch.parent_version]),
  ]))].sort((left, right) => left - right)
  const left = 180
  const top = 72
  const columnGap = 190
  const rowGap = 132
  const nodeWidth = 142
  const width = Math.max(700, left + Math.max(versions.length - 1, 0) * columnGap + nodeWidth / 2 + 36)
  const height = top + Math.max(catalog.branches.length - 1, 0) * rowGap + 92
  const column = new Map(versions.map((version, index) => [version, index]))
  const branchRow = new Map(catalog.branches.map((branch, index) => [branch.name, index]))
  const point = (branch: string, version: number) => ({
    x: left + (column.get(version) ?? 0) * columnGap,
    y: top + (branchRow.get(branch) ?? 0) * rowGap,
  })

  return (
    <div className="lineage-frame">
      <div className="lineage-legend">
        <span><i className="legend-line" />version history</span>
        <span><i className="legend-fork" />branch fork</span>
        <span>select a node to inspect that snapshot</span>
      </div>
      <div className="lineage-scroll">
        <div className="lineage-canvas" style={{ width, height }}>
          {versions.map((version) => (
            <span
              className="lineage-column-label"
              key={version}
              style={{ left: left + (column.get(version) ?? 0) * columnGap }}
            >
              v{version}
            </span>
          ))}
          {catalog.branches.map((branch, index) => (
            <div className="lineage-branch-label" key={branch.name} style={{ top: top + index * rowGap }}>
              <GitBranch size={13} />
              <strong>{branch.name}</strong>
            </div>
          ))}
          <svg className="lineage-edges" width={width} height={height} aria-hidden="true">
            <defs>
              <marker id="fork-arrow" viewBox="0 0 8 8" refX="6" refY="4" markerWidth="6" markerHeight="6" orient="auto">
                <path d="M 0 0 L 8 4 L 0 8 z" />
              </marker>
            </defs>
            {catalog.branches.flatMap((branch) => {
              const branchVersions = [...branch.versions].sort((leftVersion, rightVersion) => leftVersion.version - rightVersion.version)
              return branchVersions.slice(1).map((version, index) => {
                const from = point(branch.name, branchVersions[index].version)
                const to = point(branch.name, version.version)
                return (
                  <path
                    className="history-edge"
                    d={`M ${from.x + nodeWidth / 2} ${from.y} L ${to.x - nodeWidth / 2} ${to.y}`}
                    key={`${branch.name}-${version.version}`}
                  />
                )
              })
            })}
            {catalog.branches.map((branch) => {
              if (branch.parent_version === null || branch.versions.length === 0) return null
              const parentBranch = branch.parent_branch ?? 'main'
              if (!branchRow.has(parentBranch)) return null
              const firstVersion = [...branch.versions].sort((leftVersion, rightVersion) => leftVersion.version - rightVersion.version)[0]
              const from = point(parentBranch, branch.parent_version)
              const to = point(branch.name, firstVersion.version)
              const direction = to.y >= from.y ? 1 : -1
              const startY = from.y + direction * 34
              const endY = to.y - direction * 42
              const middleY = (startY + endY) / 2
              return (
                <g key={`fork-${branch.name}`}>
                  <circle className="fork-origin" cx={from.x} cy={startY} r="3" />
                  <path
                    className="fork-edge"
                    d={`M ${from.x} ${startY} C ${from.x} ${middleY}, ${to.x} ${middleY}, ${to.x} ${endY}`}
                    markerEnd="url(#fork-arrow)"
                  />
                </g>
              )
            })}
          </svg>
          {catalog.branches.flatMap((branch) => {
            const latestVersion = Math.max(...branch.versions.map((version) => version.version))
            return branch.versions.map((version) => {
              const reference = branch.name === 'main' ? `${version.version}` : `${branch.name}:${version.version}`
              const position = point(branch.name, version.version)
              return (
                <div className="lineage-node-wrap" key={`${branch.name}-${version.version}`} style={{ left: position.x, top: position.y }}>
                  <button
                    className={`lineage-node ${connecting === reference ? 'connecting' : ''}`}
                    onClick={() => onSelect(reference)}
                    disabled={Boolean(connecting)}
                    title={`${branch.name} at version ${version.version} · ${new Date(version.timestamp).toLocaleString()}`}
                  >
                    <span className="lineage-node-title"><History size={12} />version {version.version}</span>
                    {version.version === latestVersion && <span className="lineage-latest">latest</span>}
                    <span className="lineage-rows">
                      {version.total_rows === null ? 'rows unavailable' : `${version.total_rows.toLocaleString()} rows`}
                    </span>
                    <time>{new Date(version.timestamp).toLocaleDateString()}</time>
                    {connecting === reference && <RefreshCw className="spin lineage-loading" size={13} />}
                  </button>
                  {version.tags.length > 0 && (
                    <div className="lineage-tags">
                      {version.tags.map((tag) => (
                        <button key={tag} onClick={() => onSelect(`tag:${tag}`)} disabled={Boolean(connecting)}>
                          <Tag size={10} />{tag}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              )
            })
          })}
        </div>
      </div>
    </div>
  )
}

function ReferenceBrowser({
  catalog,
  overlay,
  onConnected,
  onBack,
}: {
  catalog: ReferenceCatalog
  overlay?: boolean
  onConnected: (dataset: ConnectedDataset) => void
  onBack: () => void
}) {
  const [error, setError] = useState('')
  const [connecting, setConnecting] = useState('')

  const select = (reference: string) => {
    setConnecting(reference)
    setError('')
    fetch('/api/dataset/connect', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ uri: catalog.uri, reference }),
    })
      .then(async (response) => {
        const body = await response.json()
        if (!response.ok) throw new Error(body.error ?? response.statusText)
        return body as ConnectedDataset
      })
      .then(onConnected)
      .catch((reason: Error) => setError(reason.message))
      .finally(() => setConnecting(''))
  }

  return (
    <div className={overlay ? 'connector-overlay' : 'connector-screen'}>
      <section className="connector-card reference-browser">
        <div className="connector-brand"><span className="brand-mark"><GitBranch /></span><strong>Dataset reference</strong></div>
        <span className="eyebrow">{overlay ? 'Switch snapshot' : 'Choose a snapshot'}</span>
        <h1>Snapshot lineage</h1>
        <p className="reference-dataset-uri">{catalog.uri}</p>
        <LineageGraph catalog={catalog} connecting={connecting} onSelect={select} />
        {error && <div className="connector-error"><CircleAlert size={15} />{error}</div>}
        <button className="connector-cancel" onClick={onBack}>{overlay ? 'Cancel' : 'Back to dataset location'}</button>
      </section>
    </div>
  )
}

function App() {
  const [connection, setConnection] = useState<ConnectedDataset>()
  const [catalog, setCatalog] = useState<ReferenceCatalog>()
  const [pendingCatalog, setPendingCatalog] = useState<ReferenceCatalog>()
  const [files, setFiles] = useState<FileEntry[]>([])
  const [selection, setSelection] = useState<Selection>({ type: 'overview' })
  const [error, setError] = useState('')
  const [mode, setMode] = useState<'infra' | 'user'>('user')
  const [showConnector, setShowConnector] = useState(true)
  const [showReference, setShowReference] = useState(false)
  const info = connection?.dataset
  const connectionId = connection?.connection_id
  const tree = useMemo(() => buildTree(files), [files])

  useEffect(() => {
    const reconnect = () => {
      setShowReference(false)
      setShowConnector(true)
    }
    window.addEventListener('lance-connection-expired', reconnect)
    return () => window.removeEventListener('lance-connection-expired', reconnect)
  }, [])

  const connected = async (nextConnection: ConnectedDataset) => {
    try {
      const response = await fetch(connectedUrl('/api/files', nextConnection.connection_id))
      await requireOk(response)
      const entries = await response.json() as FileEntry[]
      setConnection(nextConnection)
      if (pendingCatalog) setCatalog(pendingCatalog)
      setPendingCatalog(undefined)
      setFiles(entries)
      setSelection({ type: 'overview' })
      setShowConnector(false)
      setShowReference(false)
      setError('')
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  const discovered = (nextCatalog: ReferenceCatalog) => {
    setPendingCatalog(nextCatalog)
    setShowConnector(false)
    if (info) setShowReference(true)
  }

  if (error) {
    return <main className="fatal"><CircleAlert /><h1>Unable to inspect dataset</h1><p>{error}</p><button onClick={() => { setError(''); setShowConnector(true) }}>Choose another dataset</button></main>
  }
  if (!info && showConnector) {
    return <DatasetConnector onDiscovered={discovered} />
  }
  if (!info && pendingCatalog) {
    return (
      <ReferenceBrowser
        catalog={pendingCatalog}
        onConnected={connected}
        onBack={() => { setPendingCatalog(undefined); setShowConnector(true) }}
      />
    )
  }
  if (!info || !connectionId) {
    return <DatasetConnector onDiscovered={discovered} />
  }

  const selectedFile = selection.type === 'file' ? selection.file : undefined
  return (
    <div className={`app-shell ${mode === 'user' ? 'user-mode' : ''}`}>
      <header className="topbar">
        <button className="brand" onClick={() => setSelection({ type: 'overview' })}>
          <span className="brand-mark"><Database size={18} /></span>
          <span><strong>Lance</strong> Inspector</span>
        </button>
        <div className="dataset-uri"><span className="status-dot" />{info.uri}</div>
        <div className="top-meta">
          <button className="change-dataset" onClick={() => setShowConnector(true)}><HardDrive size={14} />Open dataset</button>
          <button className="reference-switch" onClick={() => setShowReference(true)} title="Choose a branch, version, or tag">
            <GitBranch size={14} />{info.branch} · version {info.version}<ChevronDown size={13} />
          </button>
          <div className="mode-switch" role="group" aria-label="Inspector mode">
            <button className={mode === 'infra' ? 'active' : ''} onClick={() => setMode('infra')} title="Show storage internals">
              <Braces size={13} />Infra
            </button>
            <button className={mode === 'user' ? 'active' : ''} onClick={() => setMode('user')} title="Show dataset rows only">
              <Rows3 size={13} />User
            </button>
          </div>
        </div>
      </header>
      {mode === 'infra' && (
        <aside className="sidebar">
          <div className="sidebar-label">Views</div>
          <nav className="sidebar-views">
            <button className={`dataset-root ${selection.type === 'overview' ? 'selected' : ''}`} onClick={() => setSelection({ type: 'overview' })}>
              <Database size={16} /><span>Dataset root</span><small>{info.rows} rows</small>
            </button>
            <button className={`dataset-root ${selection.type === 'sql' ? 'selected' : ''}`} onClick={() => setSelection({ type: 'sql' })}>
              <Search size={16} /><span>SQL query</span><small>read only</small>
            </button>
          </nav>
          <div className="sidebar-label">Storage files <span>{files.length}</span></div>
          <nav className="file-tree">
            {tree.map((node) => (
              <TreeItem
                key={node.path}
                node={node}
                selected={selectedFile?.path}
                onSelect={(file) => setSelection({ type: 'file', file })}
              />
            ))}
          </nav>
          <div className="sidebar-footer"><Braces size={14} /><span>Lance format inspector</span></div>
        </aside>
      )}
      <main className="content">
        {mode === 'user' ? (
          <DatasetQueryView info={info} mode="user" connectionId={connectionId} />
        ) : (
          <>
            {selection.type === 'overview' && <Overview info={info} />}
            {selection.type === 'sql' && <DatasetQueryView info={info} mode="infra" connectionId={connectionId} />}
            {selectedFile?.kind === 'manifest' && <ManifestView info={info} file={selectedFile} />}
            {selectedFile?.kind === 'data' && <DataView info={info} file={selectedFile} connectionId={connectionId} />}
            {selectedFile?.kind === 'deletion' && <DeletionFileView info={info} file={selectedFile} />}
            {selectedFile?.kind === 'transaction' && <TransactionFileView file={selectedFile} connectionId={connectionId} />}
            {selectedFile && !['manifest', 'data', 'deletion', 'transaction'].includes(selectedFile.kind) && (
              <RawFileView file={selectedFile} connectionId={connectionId} />
            )}
          </>
        )}
      </main>
      {showConnector && <DatasetConnector currentUri={info.uri} onDiscovered={discovered} onCancel={() => setShowConnector(false)} />}
      {showReference && (pendingCatalog ?? catalog) && (
        <ReferenceBrowser
          catalog={(pendingCatalog ?? catalog)!}
          overlay
          onConnected={connected}
          onBack={() => { setPendingCatalog(undefined); setShowReference(false) }}
        />
      )}
    </div>
  )
}

export default App
