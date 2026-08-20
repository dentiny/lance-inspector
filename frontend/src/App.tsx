import { useEffect, useRef, useState } from 'react'
import {
  Braces,
  ChevronDown,
  CircleAlert,
  Database,
  GitBranch,
  HardDrive,
  Rows3,
  Search,
} from 'lucide-react'
import { connectedUrl, errorMessage, isAbortError, requestJson } from './api'
import { DatasetConnector, ReferenceBrowser } from './components/DatasetConnector'
import { FileDataView } from './components/DataViews'
import { FileTree } from './components/FileTree'
import { Overview } from './components/Overview'
import { DatasetQueryView } from './components/SqlQueryView'
import type { ConnectedDataset, FileEntry, FilesPage, ReferenceCatalog, Selection } from './types'

const FILES_PAGE_SIZE = 500

function App() {
  const [connection, setConnection] = useState<ConnectedDataset>()
  const [catalog, setCatalog] = useState<ReferenceCatalog>()
  const [pendingCatalog, setPendingCatalog] = useState<ReferenceCatalog>()
  const [files, setFiles] = useState<FileEntry[]>([])
  const [nextFileOffset, setNextFileOffset] = useState<number | null>(null)
  const [loadingFiles, setLoadingFiles] = useState(false)
  const [selection, setSelection] = useState<Selection>({ type: 'overview' })
  const [error, setError] = useState('')
  const [mode, setMode] = useState<'infra' | 'user'>('user')
  const [showConnector, setShowConnector] = useState(true)
  const [showReference, setShowReference] = useState(false)
  const fileRequest = useRef<AbortController | undefined>(undefined)
  const fileGeneration = useRef(0)
  const loadingFilesRef = useRef(false)
  const info = connection?.dataset
  const connectionId = connection?.connection_id

  useEffect(() => {
    const reconnect = () => {
      setShowReference(false)
      setShowConnector(true)
    }
    window.addEventListener('lance-connection-expired', reconnect)
    return () => window.removeEventListener('lance-connection-expired', reconnect)
  }, [])

  const loadFiles = async (nextConnection: ConnectedDataset, offset: number, append: boolean) => {
    if (!append) fileRequest.current?.abort()
    const controller = new AbortController()
    fileRequest.current = controller
    const generation = append ? fileGeneration.current : ++fileGeneration.current
    loadingFilesRef.current = true
    setLoadingFiles(append)
    try {
      const page = await requestJson<FilesPage>(
        connectedUrl(`/api/files?offset=${offset}&limit=${FILES_PAGE_SIZE}`, nextConnection.connection_id),
        { signal: controller.signal },
      )
      if (generation !== fileGeneration.current) return
      if (append) {
        setFiles((current) => [...current, ...page.entries])
        setNextFileOffset(page.next_offset)
        return
      }
      setConnection(nextConnection)
      if (pendingCatalog) setCatalog(pendingCatalog)
      setPendingCatalog(undefined)
      setFiles(page.entries)
      setNextFileOffset(page.next_offset)
      setSelection({ type: 'overview' })
      setShowConnector(false)
      setShowReference(false)
      setError('')
    } catch (reason) {
      if (!isAbortError(reason)) setError(errorMessage(reason))
    } finally {
      if (generation === fileGeneration.current) {
        fileRequest.current = undefined
        loadingFilesRef.current = false
        setLoadingFiles(false)
      }
    }
  }

  const connected = (nextConnection: ConnectedDataset) => loadFiles(nextConnection, 0, false)

  const loadMoreFiles = () => {
    if (!connection || nextFileOffset === null || loadingFilesRef.current) return
    void loadFiles(connection, nextFileOffset, true)
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
          <FileTree
            files={files}
            selected={selectedFile?.path}
            hasMore={nextFileOffset !== null}
            loading={loadingFiles}
            onSelect={(file) => setSelection({ type: 'file', file })}
            onLoadMore={() => void loadMoreFiles()}
          />
          <div className="sidebar-footer"><Braces size={14} /><span>Lance format inspector</span></div>
        </aside>
      )}
      <main className="content">
        {mode === 'user' ? (
          <DatasetQueryView info={info} mode="user" connectionId={connectionId} />
        ) : (
          <>
            {selection.type === 'overview' && <Overview info={info} catalog={catalog} />}
            {selection.type === 'sql' && <DatasetQueryView info={info} mode="infra" connectionId={connectionId} />}
            {selectedFile && <FileDataView info={info} file={selectedFile} connectionId={connectionId} />}
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
