import { useState } from 'react'
import { Activity, Braces, FileArchive, GitBranch } from 'lucide-react'
import { connectedUrl, useJsonResource } from '../api'
import { formatBytes } from '../format'
import type { DatasetInfo, FileEntry, RowsResponse, TransactionInfo } from '../types'
import { AsyncStatus, PageHeading } from '../ui'
import { DeletionGrid } from './DeletionGrid'
import { RowsTable } from './RowsTable'
import { StatCard } from './StatCard'

function RecordFields({
  record,
  preformatted,
  emptyAsNull = false,
}: {
  record: Record<string, unknown>
  preformatted?: string
  emptyAsNull?: boolean
}) {
  return (
    <div className={`record-fields${preformatted ? ' scroll-record-values' : ''}`}>
      {Object.entries(record).map(([key, value]) => (
        <div className="record-field" key={key}>
          <span>{key.replaceAll('_', ' ')}</span>
          {typeof value === 'object' && value !== null
            ? <pre>{JSON.stringify(value, null, 2)}</pre>
            : key === preformatted
              ? <pre>{String(value)}</pre>
              : <strong>{value == null || (emptyAsNull && value === '') ? '—' : String(value)}</strong>}
        </div>
      ))}
    </div>
  )
}

function ManifestView({ info, file }: { info: DatasetInfo; file: FileEntry }) {
  return (
    <div className="page">
      <PageHeading
        eyebrow="Decoded protobuf"
        title="Manifest"
        subtitle={file.path}
        badge={<span className="count-badge">{formatBytes(file.size)}</span>}
      />
      <section className="panel"><RecordFields record={info.manifest} /></section>
    </div>
  )
}

function RowsPanel({
  connectionId,
  title = 'Row preview',
}: {
  connectionId: string
  title?: string
}) {
  const [offset, setOffset] = useState(0)
  const { data, error } = useJsonResource<RowsResponse>(
    connectedUrl(`/api/rows?offset=${offset}&limit=20`, connectionId),
  )

  return (
    <section className="panel data-panel">
      <div className="panel-title">
        <div><span className="eyebrow">Live dataset scan</span><h2>{title}</h2></div>
        {data && <span className="count-badge">{offset + 1}–{Math.min(offset + data.rows.length, data.total)} of {data.total}</span>}
      </div>
      {(error || !data) && <AsyncStatus error={error} loading="Scanning Lance rows…" />}
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
      <PageHeading
        eyebrow="Data file · first 20 rows"
        title={file.path.split('/').at(-1)}
        subtitle={file.path}
        badge={<span className="count-badge">{formatBytes(file.size)}</span>}
      />
      {fragment?.deletion && <DeletionGrid fragment={fragment} />}
      <RowsPanel key={file.path} connectionId={connectionId} />
    </div>
  )
}

function DeletionFileView({ info, file }: { info: DatasetInfo; file: FileEntry }) {
  const fragment = info.fragments.find((item) => item.deletion?.path === file.path)
  return (
    <div className="page">
      <PageHeading eyebrow="Physical tombstones" title="Deletion vector" subtitle={file.path} />
      {fragment ? <DeletionGrid fragment={fragment} /> : <div className="empty-state">This deletion file is not referenced by the active manifest.</div>}
    </div>
  )
}

function TransactionFileView({ file, connectionId }: { file: FileEntry; connectionId: string }) {
  const { data: transaction, error } = useJsonResource<TransactionInfo>(
    connectedUrl(`/api/transaction?path=${encodeURIComponent(file.path)}`, connectionId),
  )

  return (
    <div className="page">
      <PageHeading
        eyebrow="Decoded protobuf"
        title="Transaction"
        subtitle={file.path}
        badge={transaction && <span className="count-badge">{transaction.operation_type}</span>}
      />
      {(error || !transaction) && <AsyncStatus error={error} loading="Decoding transaction…" />}
      {transaction && (
        <>
          <div className="stats-grid transaction-stats">
            <StatCard icon={<Activity />} label="Operation" value={transaction.operation_type} detail="protobuf oneof" />
            <StatCard icon={<GitBranch />} label="Read version" value={`v${transaction.read_version}`} detail="transaction base" />
            <StatCard icon={<Braces />} label="UUID" value={transaction.uuid.slice(0, 8)} detail={transaction.uuid} />
            <StatCard icon={<FileArchive />} label="File size" value={formatBytes(file.size)} detail={transaction.tag ?? 'no version tag'} />
          </div>
          <section className="panel">
            <div className="panel-title"><div><span className="eyebrow">Operation payload</span><h2>{transaction.operation_type}</h2></div></div>
            <RecordFields record={transaction.operation} preformatted="details" emptyAsNull />
          </section>
          {Object.keys(transaction.properties).length > 0 && (
            <section className="panel">
              <div className="panel-title"><div><span className="eyebrow">Commit metadata</span><h2>Properties</h2></div></div>
              <RecordFields record={transaction.properties} />
            </section>
          )}
        </>
      )}
    </div>
  )
}

function RawFileView({ file, connectionId }: { file: FileEntry; connectionId: string }) {
  const { data: preview, error } = useJsonResource<{
    content: string
    format: string
    truncated: boolean
  }>(connectedUrl(`/api/file?path=${encodeURIComponent(file.path)}`, connectionId))
  return (
    <div className="page">
      <PageHeading
        eyebrow={`${file.kind} file`}
        title={file.path.split('/').at(-1)}
        subtitle={file.path}
        badge={<span className="count-badge">{formatBytes(file.size)}</span>}
      />
      {(error || !preview) && <AsyncStatus error={error} loading="Reading file…" />}
      {preview && (
        <section className="panel raw-panel">
          <div className="panel-title"><h2>{preview.format === 'hex' ? 'Hex preview' : 'Text preview'}</h2>{preview.truncated && <span className="count-badge">first 64 KB</span>}</div>
          <pre>{preview.content}</pre>
        </section>
      )}
    </div>
  )
}

export function FileDataView({
  info,
  file,
  connectionId,
}: {
  info: DatasetInfo
  file: FileEntry
  connectionId: string
}) {
  if (file.kind === 'manifest') return <ManifestView info={info} file={file} />
  if (file.kind === 'data') return <DataView info={info} file={file} connectionId={connectionId} />
  if (file.kind === 'deletion') return <DeletionFileView info={info} file={file} />
  if (file.kind === 'transaction') return <TransactionFileView file={file} connectionId={connectionId} />
  return <RawFileView file={file} connectionId={connectionId} />
}
