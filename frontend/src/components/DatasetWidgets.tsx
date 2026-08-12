import type { ReactNode } from 'react'
import type { Fragment } from '../types'

export function StatCard({
  icon,
  label,
  value,
  detail,
}: {
  icon: ReactNode
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

export function DeletionGrid({ fragment }: { fragment: Fragment }) {
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
        <div><strong>{fragment.deletion.count}</strong><span>deleted rows</span></div>
        <div>
          <strong>{physical ? ((fragment.deletion.count / physical) * 100).toFixed(2) : '0'}%</strong>
          <span>of physical rows</span>
        </div>
        <div><strong>{fragment.deletion.file_type}</strong><span>encoding</span></div>
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
