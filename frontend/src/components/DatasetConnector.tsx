import { useState } from 'react'
import {
  ArrowRight,
  CircleAlert,
  Database,
  GitBranch,
  HardDrive,
  RefreshCw,
} from 'lucide-react'
import { errorMessage, requestJson } from '../api'
import type { ConnectedDataset, ReferenceCatalog } from '../types'
import { LineageGraph } from './LineageGraph'

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
    requestJson<ReferenceCatalog>('/api/dataset/references', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ uri: location }),
    })
      .then(onDiscovered)
      .catch((reason: unknown) => setError(errorMessage(reason)))
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
    requestJson<ConnectedDataset>('/api/dataset/connect', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ discovery_id: catalog.discovery_id, reference }),
    })
      .then(onConnected)
      .catch((reason: unknown) => setError(errorMessage(reason)))
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
