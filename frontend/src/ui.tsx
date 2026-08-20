import type { ReactNode } from 'react'
import { CircleAlert, RefreshCw } from 'lucide-react'

export function PageHeading({
  eyebrow,
  title,
  subtitle,
  badge,
}: {
  eyebrow: ReactNode
  title: ReactNode
  subtitle?: ReactNode
  badge?: ReactNode
}) {
  return (
    <div className="page-heading">
      <div>
        <span className="eyebrow">{eyebrow}</span>
        <h1>{title}</h1>
        {subtitle !== undefined && <p>{subtitle}</p>}
      </div>
      {badge}
    </div>
  )
}

export function AsyncStatus({
  error,
  loading,
}: {
  error: string
  loading: ReactNode
}) {
  return error
    ? <div className="error-state"><CircleAlert />{error}</div>
    : <div className="loading-state"><RefreshCw className="spin" />{loading}</div>
}
