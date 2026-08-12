import { useMemo, useState } from 'react'
import {
  ChevronDown,
  ChevronRight,
  File,
  FileArchive,
  FileJson,
  FileX2,
  Folder,
  FolderOpen,
  RefreshCw,
  Rows3,
  Search,
} from 'lucide-react'
import { formatBytes } from '../format'
import type { FileEntry } from '../types'

type TreeNode = {
  name: string
  path: string
  children: TreeNode[]
  file?: FileEntry
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

export function FileTree({
  files,
  selected,
  hasMore,
  loading,
  onSelect,
  onLoadMore,
}: {
  files: FileEntry[]
  selected?: string
  hasMore: boolean
  loading: boolean
  onSelect: (file: FileEntry) => void
  onLoadMore: () => void
}) {
  const tree = useMemo(() => buildTree(files), [files])
  return (
    <nav className="file-tree">
      {tree.map((node) => (
        <TreeItem key={node.path} node={node} selected={selected} onSelect={onSelect} />
      ))}
      {hasMore && (
        <button className="load-more-files" onClick={onLoadMore} disabled={loading}>
          {loading ? <><RefreshCw className="spin" size={13} />Loading files…</> : 'Load more files'}
        </button>
      )}
    </nav>
  )
}
