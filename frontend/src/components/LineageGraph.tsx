import { GitBranch, History, RefreshCw, Tag } from 'lucide-react'
import type { ReferenceCatalog } from '../types'

export function LineageGraph({
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
                    title={`${branch.name} at version ${version.version}`}
                  >
                    <span className="lineage-node-title"><History size={12} />version {version.version}</span>
                    {version.version === latestVersion && <span className="lineage-latest">latest</span>}
                    <span className="lineage-rows">
                      {version.total_rows === null ? 'rows unavailable' : `${version.total_rows.toLocaleString()} rows`}
                    </span>
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
