import { ChevronDown, Image as ImageIcon, Search } from 'lucide-react'
import { formatBytes } from '../format'
import type { DatasetInfo } from '../types'

export function DatasetStructure({ info }: { info: DatasetInfo }) {
  return (
    <>
      <details className="panel collapsible-panel">
        <summary className="panel-title">
          <div><span className="eyebrow">Logical model</span><h2>Schema</h2></div>
          <span className="panel-summary-meta">
            <span className="count-badge">{info.schema.length} fields</span>
            <ChevronDown className="panel-chevron" size={15} />
          </span>
        </summary>
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
      </details>
      <details className="panel collapsible-panel">
        <summary className="panel-title">
          <div><span className="eyebrow">Query acceleration</span><h2>Indices</h2></div>
          <span className="panel-summary-meta">
            <span className="count-badge">{info.indices.length} indices</span>
            <ChevronDown className="panel-chevron" size={15} />
          </span>
        </summary>
        {info.indices.length === 0
          ? <div className="empty-state">This snapshot has no indices.</div>
          : (
            <div className="index-list">
              {info.indices.map((index) => (
                <div className="index-card" key={index.name}>
                  <div className="index-heading">
                    <Search size={15} />
                    <strong>{index.name}</strong>
                    <span className="index-type">{index.index_type}</span>
                  </div>
                  <div className="index-metrics">
                    <span>{index.fields.join(', ') || 'no fields'}</span>
                    <span>{index.rows_indexed.toLocaleString()} rows</span>
                    <span>{index.segment_count} segment{index.segment_count === 1 ? '' : 's'}</span>
                    <span>{index.total_size_bytes === null ? 'size unavailable' : formatBytes(index.total_size_bytes)}</span>
                  </div>
                  {index.details !== null && (
                    <details className="index-details">
                      <summary>Index details</summary>
                      <pre>{JSON.stringify(index.details, null, 2)}</pre>
                    </details>
                  )}
                </div>
              ))}
            </div>
          )}
      </details>
    </>
  )
}
