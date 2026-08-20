import { useEffect, useState } from 'react'

export const connectedUrl = (path: string, connectionId: string) => {
  const separator = path.includes('?') ? '&' : '?'
  return `${path}${separator}connection_id=${encodeURIComponent(connectionId)}`
}

export class HttpError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = 'HttpError'
    this.status = status
  }
}

export async function requireOk(response: Response) {
  if (response.ok) return response
  const body = await response.json().catch(() => ({ error: response.statusText }))
  if (response.status === 410) {
    window.dispatchEvent(new Event('lance-connection-expired'))
  }
  throw new HttpError(body.error ?? response.statusText, response.status)
}

export const errorMessage = (reason: unknown) => reason instanceof Error ? reason.message : String(reason)
export const isAbortError = (reason: unknown) => reason instanceof DOMException && reason.name === 'AbortError'

export async function requestJson<T>(input: RequestInfo | URL, init?: RequestInit) {
  const response = await fetch(input, init)
  await requireOk(response)
  return response.json() as Promise<T>
}

export function useJsonResource<T>(url: string) {
  const [data, setData] = useState<T>()
  const [error, setError] = useState('')
  useEffect(() => {
    const controller = new AbortController()
    setData(undefined)
    setError('')
    requestJson<T>(url, { signal: controller.signal })
      .then(setData)
      .catch((reason: unknown) => {
        if (!isAbortError(reason)) setError(errorMessage(reason))
      })
    return () => controller.abort()
  }, [url])
  return { data, error }
}
