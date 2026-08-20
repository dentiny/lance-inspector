import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
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

type MutableTreeNode = Omit<TreeNode, 'children'> & {
  children: Map<string, MutableTreeNode>
}

function buildTree(files: FileEntry[]): TreeNode[] {
  const root: MutableTreeNode = { name: '', path: '', children: new Map() }
  for (const file of files) {
    let current = root
    let path = ''
    const parts = file.path.split('/')
    parts.forEach((part, index) => {
      path = path ? `${path}/${part}` : part
      let node = current.children.get(part)
      if (!node) {
        node = { name: part, path, children: new Map() }
        current.children.set(part, node)
      }
      if (index === parts.length - 1) node.file = file
      current = node
    })
  }

  const materialize = (nodes: Map<string, MutableTreeNode>): TreeNode[] => (
    [...nodes.values()]
      .sort((left, right) => {
        if (!!left.file !== !!right.file) return left.file ? 1 : -1
        return left.name.localeCompare(right.name)
      })
      .map((node) => ({
        name: node.name,
        path: node.path,
        children: materialize(node.children),
        file: node.file,
      }))
  )
  return materialize(root.children)
}

function fileIcon(kind: FileEntry['kind']) {
  if (kind === 'manifest') return <FileJson size={15} />
  if (kind === 'data') return <Rows3 size={15} />
  if (kind === 'deletion') return <FileX2 size={15} />
  if (kind === 'index') return <Search size={15} />
  if (kind === 'transaction') return <FileArchive size={15} />
  return <File size={15} />
}

type VisibleNode = {
  node: TreeNode
  depth: number
}

function flattenTree(tree: TreeNode[], closedPaths: Set<string>) {
  const visible: VisibleNode[] = []
  const visit = (nodes: TreeNode[], depth: number) => {
    for (const node of nodes) {
      visible.push({ node, depth })
      if (!node.file && !closedPaths.has(node.path)) visit(node.children, depth + 1)
    }
  }
  visit(tree, 0)
  return visible
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
  const scrollContainer = useRef<HTMLElement>(null)
  const [closedPaths, setClosedPaths] = useState(() => new Set<string>())
  const tree = useMemo(() => buildTree(files), [files])
  const visibleNodes = useMemo(() => flattenTree(tree, closedPaths), [tree, closedPaths])
  const virtualizer = useVirtualizer({
    count: visibleNodes.length,
    getScrollElement: () => scrollContainer.current,
    estimateSize: () => 28,
    getItemKey: (index) => visibleNodes[index]?.node.path ?? index,
    overscan: 12,
  })
  const requestMoreIfNeeded = useCallback((element: HTMLElement) => {
    if (hasMore && !loading && element.scrollHeight - element.scrollTop - element.clientHeight < 200) {
      onLoadMore()
    }
  }, [hasMore, loading, onLoadMore])
  useEffect(() => {
    const element = scrollContainer.current
    if (!element) return
    requestMoreIfNeeded(element)
    const resizeObserver = new ResizeObserver(() => requestMoreIfNeeded(element))
    resizeObserver.observe(element)
    return () => resizeObserver.disconnect()
  }, [visibleNodes.length, requestMoreIfNeeded])

  const toggleDirectory = (path: string) => {
    setClosedPaths((current) => {
      const next = new Set(current)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }

  return (
    <nav
      ref={scrollContainer}
      className="file-tree"
      onScroll={(event) => requestMoreIfNeeded(event.currentTarget)}
    >
      <div className="tree-virtual-content" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((virtualRow) => {
          const { node, depth } = visibleNodes[virtualRow.index]
          const directory = !node.file
          const open = directory && !closedPaths.has(node.path)
          return (
            <div
              className="tree-node"
              key={virtualRow.key}
              style={{ transform: `translateY(${virtualRow.start}px)` }}
            >
              <button
                className={`tree-row ${selected === node.path ? 'selected' : ''}`}
                onClick={() => (directory ? toggleDirectory(node.path) : onSelect(node.file!))}
                style={{ paddingLeft: 2 + depth * 14 }}
                title={node.path}
              >
                <span className="tree-chevron">
                  {directory ? open ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : null}
                </span>
                {directory ? open ? <FolderOpen size={15} /> : <Folder size={15} /> : fileIcon(node.file!.kind)}
                <span className="tree-name">{node.name}</span>
                {node.file && <span className="tree-size">{formatBytes(node.file.size)}</span>}
              </button>
            </div>
          )
        })}
      </div>
      {loading && <div className="tree-loading"><RefreshCw className="spin" size={13} />Loading files…</div>}
    </nav>
  )
}
