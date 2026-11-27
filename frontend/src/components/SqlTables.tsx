import { useEffect, useState } from 'react'
import { connectedUrl, errorMessage, requestJson, requireOk } from '../api'

type SqlTable = {
  name: string
  uri: string
  validated_version: number
  columns: string[]
}

export function SqlTables({ connectionId }: { connectionId: string }) {
  const [tables, setTables] = useState<SqlTable[]>([])
  const [open, setOpen] = useState(false)
  const [name, setName] = useState('')
  const [uri, setUri] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [loaded, setLoaded] = useState(false)
  const [reload, setReload] = useState(0)

  useEffect(() => {
    const controller = new AbortController()
    setError('')
    void requestJson<SqlTable[]>(connectedUrl('/api/sql/tables', connectionId), { signal: controller.signal })
      .then((rows) => { if (!controller.signal.aborted) { setTables(rows); setLoaded(true) } })
      .catch((reason) => { if (!controller.signal.aborted) setError(errorMessage(reason)) })
    return () => controller.abort()
  }, [connectionId, reload])

  const attach = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError('')
    try {
      const table = await requestJson<SqlTable>(connectedUrl('/api/sql/tables', connectionId), {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, uri }),
      })
      setTables((current) => [...current, table].sort((a, b) => a.name.localeCompare(b.name)))
      setName('')
      setUri('')
      setOpen(false)
    } catch (reason) { setError(errorMessage(reason)) }
    finally { setBusy(false) }
  }

  const detach = async (table: SqlTable) => {
    setBusy(true)
    setError('')
    try {
      await requireOk(await fetch(connectedUrl(`/api/sql/tables/${encodeURIComponent(table.name)}`, connectionId), { method: 'DELETE' }))
      setTables((current) => current.filter((item) => item.name !== table.name))
    } catch (reason) { setError(errorMessage(reason)) }
    finally { setBusy(false) }
  }

  return <div className="sql-tables">
    <div className="sql-table-heading">
      <strong>Query tables</strong>
      <button type="button" disabled={!loaded || busy} onClick={() => setOpen((value) => !value)}>
        {open ? 'Close' : 'Add dataset'}
      </button>
    </div>
    <p><code>dataset</code> is the selected snapshot. Added tables open at latest when each query starts.</p>
    {tables.map((table) => <div className="sql-table-entry" key={table.name}>
      <div><code>{table.name}</code><span title={table.uri}>{table.uri}</span>
        <small>Validated at v{table.validated_version} · {table.columns.length} columns</small></div>
      <button type="button" disabled={busy} aria-label={`Remove ${table.name}`} onClick={() => void detach(table)}>Remove</button>
    </div>)}
    {open && <form className="sql-attach-form" onSubmit={(event) => void attach(event)}>
      <label>Dataset location<input required value={uri} onChange={(event) => setUri(event.target.value)} placeholder="s3://bucket/pack/recordings.lance" /></label>
      <label>Use as<input required maxLength={63} pattern="[a-z_][a-z0-9_]*" value={name} onChange={(event) => setName(event.target.value)} placeholder="recordings" /></label>
      <button type="submit" disabled={busy || !loaded}>{busy ? 'Opening…' : 'Add table'}</button>
    </form>}
    {tables.length > 0 && <p>Join using registered names, for example <code>JOIN recordings r ON dataset.recording_id = r.recording_id</code>. Use AS for duplicate output names. Named-table results support metadata; use dataset alone for Blob previews. Remove only unregisters a table.</p>}
    {error && <p role="alert">{error} {!loaded && <button type="button" onClick={() => setReload((value) => value + 1)}>Retry tables</button>}</p>}
  </div>
}
