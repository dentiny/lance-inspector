import { useEffect, useMemo, useState } from 'react'
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
  Image as ImageIcon,
  Layers3,
  RefreshCw,
  Rows3,
  Search,
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
  version: number
  branch: string
  rows: number
  schema: SchemaField[]
  manifest: Record<string, unknown>
  fragments: Fragment[]
  branches: { name: string; parent_branch: string | null; parent_version: number }[]
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

type TransactionInfo = {
  path: string
  read_version: number
  uuid: string
  tag: string | null
  properties: Record<string, string>
  operation_type: string
  operation: Record<string, unknown>
}

type Selection = { type: 'overview' } | { type: 'file'; file: FileEntry }

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
}: {
  column: RowsResponse['media_columns'][number]
  row: Record<string, unknown>
}) {
  const rowAddress = row._rowaddr
  if (rowAddress == null) return <span>—</span>
  const mime = column.mime_column ? String(row[column.mime_column] ?? '') : ''
  const source = `/api/media/${encodeURIComponent(column.name)}/${rowAddress}?mime=${encodeURIComponent(mime)}`
  if (mime.startsWith('image/')) return <img className="media-image" src={source} loading="lazy" alt={`${column.name} preview`} />
  if (mime.startsWith('audio/')) return <audio className="media-audio" src={source} controls preload="none" />
  if (mime.startsWith('video/')) return <video className="media-video" src={source} controls preload="metadata" />
  return <a className="blob-link" href={source} target="_blank">open blob</a>
}

function DataView({ info, file }: { info: DatasetInfo; file: FileEntry }) {
  const [data, setData] = useState<RowsResponse>()
  const [offset, setOffset] = useState(0)
  const [error, setError] = useState('')
  const fragment = info.fragments.find((item) => item.files.some((candidate) => candidate.path === file.path))
  useEffect(() => {
    setData(undefined)
    setError('')
    fetch(`/api/rows?offset=${offset}&limit=20`)
      .then(async (response) => {
        if (!response.ok) throw new Error((await response.json()).error ?? response.statusText)
        return response.json() as Promise<RowsResponse>
      })
      .then(setData)
      .catch((reason: Error) => setError(reason.message))
  }, [offset, file.path])

  return (
    <div className="page wide-page">
      <div className="page-heading">
        <div><span className="eyebrow">Data file · first 20 rows</span><h1>{file.path.split('/').at(-1)}</h1><p>{file.path}</p></div>
        <span className="count-badge">{formatBytes(file.size)}</span>
      </div>
      {fragment?.deletion && <DeletionGrid fragment={fragment} />}
      <section className="panel data-panel">
        <div className="panel-title">
          <div><span className="eyebrow">Live dataset scan</span><h2>Row preview</h2></div>
          {data && <span className="count-badge">{offset + 1}–{Math.min(offset + data.rows.length, data.total)} of {data.total}</span>}
        </div>
        {error && <div className="error-state"><CircleAlert />{error}</div>}
        {!data && !error && <div className="loading-state"><RefreshCw className="spin" />Scanning Lance rows…</div>}
        {data && (
          <>
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
                      {data.media_columns.map((column) => <td key={column.name}><MediaValue column={column} row={row} /></td>)}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="pagination">
              <button disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - 20))}>Previous</button>
              <span>20 rows per page</span>
              <button disabled={offset + 20 >= data.total} onClick={() => setOffset(offset + 20)}>Next</button>
            </div>
          </>
        )}
      </section>
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

function TransactionFileView({ file }: { file: FileEntry }) {
  const [transaction, setTransaction] = useState<TransactionInfo>()
  const [error, setError] = useState('')
  useEffect(() => {
    setTransaction(undefined)
    setError('')
    fetch(`/api/transaction?path=${encodeURIComponent(file.path)}`)
      .then(async (response) => {
        if (!response.ok) throw new Error((await response.json()).error ?? response.statusText)
        return response.json() as Promise<TransactionInfo>
      })
      .then(setTransaction)
      .catch((reason: Error) => setError(reason.message))
  }, [file.path])

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

function RawFileView({ file }: { file: FileEntry }) {
  const [preview, setPreview] = useState<{ content: string; format: string; truncated: boolean }>()
  const [error, setError] = useState('')
  useEffect(() => {
    fetch(`/api/file?path=${encodeURIComponent(file.path)}`)
      .then(async (response) => {
        if (!response.ok) throw new Error((await response.json()).error ?? response.statusText)
        return response.json()
      })
      .then(setPreview)
      .catch((reason: Error) => setError(reason.message))
  }, [file.path])
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
  onConnected,
  onCancel,
}: {
  currentUri?: string
  onConnected: (dataset: DatasetInfo) => void
  onCancel?: () => void
}) {
  const [uri, setUri] = useState(currentUri ?? '')
  const [error, setError] = useState('')
  const [connecting, setConnecting] = useState(false)

  const submit = (event: React.FormEvent) => {
    event.preventDefault()
    const location = uri.trim()
    if (!location) return
    setConnecting(true)
    setError('')
    fetch('/api/dataset/connect', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ uri: location }),
    })
      .then(async (response) => {
        const body = await response.json()
        if (!response.ok) throw new Error(body.error ?? response.statusText)
        return body as DatasetInfo
      })
      .then((dataset) => {
        onConnected(dataset)
      })
      .catch((reason: Error) => setError(reason.message))
      .finally(() => setConnecting(false))
  }

  return (
    <div className={onCancel ? 'connector-overlay' : 'connector-screen'}>
      <section className="connector-card">
        <div className="connector-brand"><span className="brand-mark"><Database /></span><strong>Lance Inspector</strong></div>
        <span className="eyebrow">Open a dataset</span>
        <h1>Inspect Lance storage</h1>
        <p>Enter a local dataset path or an S3 URI accessible to this server.</p>
        <form onSubmit={submit}>
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
            <button type="submit" disabled={connecting || !uri.trim()}>
              {connecting ? <RefreshCw className="spin" size={16} /> : <ArrowRight size={16} />}
              {connecting ? 'Opening' : 'Inspect'}
            </button>
          </div>
        </form>
        {error && <div className="connector-error"><CircleAlert size={15} />{error}</div>}
        <div className="connector-hints">
          <span><strong>Local</strong> mounted paths and volumes</span>
          <span><strong>Cloud</strong> S3 with server credentials</span>
          <span><strong>Safety</strong> read-only inspection</span>
        </div>
        {onCancel && <button className="connector-cancel" onClick={onCancel}>Cancel</button>}
      </section>
    </div>
  )
}

function App() {
  const [info, setInfo] = useState<DatasetInfo>()
  const [files, setFiles] = useState<FileEntry[]>([])
  const [selection, setSelection] = useState<Selection>({ type: 'overview' })
  const [error, setError] = useState('')
  const [showConnector, setShowConnector] = useState(true)
  const tree = useMemo(() => buildTree(files), [files])

  const connected = async (dataset: DatasetInfo) => {
    try {
      const response = await fetch('/api/files')
      if (!response.ok) throw new Error(`Files API returned ${response.status}`)
      const entries = await response.json() as FileEntry[]
      setInfo(dataset)
      setFiles(entries)
      setSelection({ type: 'overview' })
      setShowConnector(false)
      setError('')
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  if (error) {
    return <main className="fatal"><CircleAlert /><h1>Unable to inspect dataset</h1><p>{error}</p><button onClick={() => { setError(''); setShowConnector(true) }}>Choose another dataset</button></main>
  }
  if (!info && showConnector) {
    return <DatasetConnector onConnected={connected} />
  }
  if (!info) {
    return <DatasetConnector onConnected={connected} />
  }

  const selectedFile = selection.type === 'file' ? selection.file : undefined
  return (
    <div className="app-shell">
      <header className="topbar">
        <button className="brand" onClick={() => setSelection({ type: 'overview' })}>
          <span className="brand-mark"><Database size={18} /></span>
          <span><strong>Lance</strong> Inspector</span>
        </button>
        <div className="dataset-uri"><span className="status-dot" />{info.uri}</div>
        <div className="top-meta">
          <button className="change-dataset" onClick={() => setShowConnector(true)}><HardDrive size={14} />Open dataset</button>
          <span><GitBranch size={14} />{info.branch}</span><span>v{info.version}</span>
        </div>
      </header>
      <aside className="sidebar">
        <button className={`dataset-root ${selection.type === 'overview' ? 'selected' : ''}`} onClick={() => setSelection({ type: 'overview' })}>
          <Database size={16} /><span>Dataset root</span><small>{info.rows} rows</small>
        </button>
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
      <main className="content">
        {selection.type === 'overview' && <Overview info={info} />}
        {selectedFile?.kind === 'manifest' && <ManifestView info={info} file={selectedFile} />}
        {selectedFile?.kind === 'data' && <DataView info={info} file={selectedFile} />}
        {selectedFile?.kind === 'deletion' && <DeletionFileView info={info} file={selectedFile} />}
        {selectedFile?.kind === 'transaction' && <TransactionFileView file={selectedFile} />}
        {selectedFile && !['manifest', 'data', 'deletion', 'transaction'].includes(selectedFile.kind) && <RawFileView file={selectedFile} />}
      </main>
      {showConnector && <DatasetConnector currentUri={info.uri} onConnected={connected} onCancel={() => setShowConnector(false)} />}
    </div>
  )
}

export default App
