import { useState } from 'react'
import {
  ArrowRight,
  CircleAlert,
  Database,
  GitBranch,
  HardDrive,
  History,
  RefreshCw,
  Tag,
} from 'lucide-react'
import type { ConnectedDataset, ReferenceCatalog } from '../types'

export function DatasetConnector({
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

export function ReferenceBrowser({
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
      body: JSON.stringify({ discovery_id: catalog.discovery_id, reference }),
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
