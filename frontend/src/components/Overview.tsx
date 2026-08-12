import { Activity, GitBranch, HardDrive, Image as ImageIcon, Layers3, Rows3 } from 'lucide-react'
import { formatBytes } from '../format'
import type { DatasetInfo } from '../types'
import { StatCard } from './DatasetWidgets'

export function Overview({ info }: { info: DatasetInfo }) {
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
        <div className="panel-title"><div><span className="eyebrow">Physical model</span><h2>Fragments</h2></div></div>
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
