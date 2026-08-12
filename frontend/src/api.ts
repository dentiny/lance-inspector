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
