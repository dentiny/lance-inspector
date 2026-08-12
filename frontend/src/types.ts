export type SchemaField = {
  name: string
  data_type: string
  nullable: boolean
  media: boolean
  metadata: Record<string, string>
}

export type Deletion = {
  path: string
  file_type: string
  read_version: number
  id: number
  count: number
  offsets: number[]
  offsets_truncated: boolean
}

export type Fragment = {
  id: number
  physical_rows: number | null
  visible_rows: number | null
  files: {
    path: string
    fields: number[]
    column_indices: number[]
    format: string
    size_bytes: number | null
    base_id: number | null
  }[]
  deletion: Deletion | null
}

export type DatasetInfo = {
  uri: string
  reference: string
  version: number
  branch: string
  rows: number
  schema: SchemaField[]
  manifest: Record<string, unknown>
  fragments: Fragment[]
  branches: { name: string; parent_branch: string | null; parent_version: number }[]
}

export type ConnectedDataset = {
  connection_id: string
  dataset: DatasetInfo
}

export type ReferenceCatalog = {
  uri: string
  branches: {
    name: string
    parent_branch: string | null
    parent_version: number | null
    versions: {
      version: number
      timestamp: string
      total_rows: number | null
      tags: string[]
    }[]
  }[]
}

export type FileEntry = {
  path: string
  size: number
  kind: 'manifest' | 'data' | 'deletion' | 'index' | 'transaction' | 'file'
  modified: string
}

export type FilesPage = {
  entries: FileEntry[]
  next_offset: number | null
}

export type RowsResponse = {
  offset: number
  limit: number
  total: number
  columns: string[]
  media_columns: { name: string; mime_column: string | null }[]
  rows: Record<string, unknown>[]
}

export type TableData = Pick<RowsResponse, 'columns' | 'media_columns' | 'rows'>

export type SqlCursorResponse = {
  cursor_id: string
  columns: string[]
  media_columns: RowsResponse['media_columns']
}

export type SqlPageResponse = {
  sequence: number
  rows: Record<string, unknown>[]
  done: boolean
  truncated: boolean
}

export type TransactionInfo = {
  path: string
  read_version: number
  uuid: string
  tag: string | null
  properties: Record<string, string>
  operation_type: string
  operation: Record<string, unknown>
}

export type Selection = { type: 'overview' } | { type: 'sql' } | { type: 'file'; file: FileEntry }
