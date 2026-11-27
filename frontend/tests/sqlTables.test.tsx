import assert from 'node:assert/strict'
import { test } from 'node:test'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost' })
Object.assign(globalThis, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true })

test('registers named datasets, reports failures, and removes only registrations', async () => {
  const { createRoot } = await import('react-dom/client')
  const { SqlTables } = await import('../src/components/SqlTables')
  const previousFetch = globalThis.fetch
  const calls: { url: string; method?: string; body?: unknown }[] = []
  let fail = true
  globalThis.fetch = async (input, init) => {
    const url = String(input)
    const body = init?.body ? JSON.parse(String(init.body)) : undefined
    calls.push({ url, method: init?.method, body })
    if (init?.method === 'POST') {
      if (fail) return Response.json({ error: 'Unable to open dataset' }, { status: 400 })
      return Response.json({ name: body.name, uri: body.uri, validated_version: 4, columns: ['recording_id'] })
    }
    if (init?.method === 'DELETE') return new Response(null, { status: 204 })
    return Response.json([])
  }
  const container = document.createElement('div')
  document.body.append(container)
  const root = createRoot(container)
  const button = (text: string) => [...container.querySelectorAll('button')].find((item) => item.textContent === text)!
  const input = async (placeholder: string, value: string) => {
    const element = container.querySelector<HTMLInputElement>(`input[placeholder="${placeholder}"]`)!
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value')!.set!.call(element, value)
      element.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
    })
  }
  try {
    await act(async () => root.render(<SqlTables connectionId="session-a" />))
    await act(async () => button('Add dataset').click())
    await input('s3://bucket/pack/recordings.lance', 's3://bucket/base.lance')
    await input('recordings', 'recordings')
    const submit = async () => act(async () => {
      container.querySelector('form')!.dispatchEvent(new dom.window.Event('submit', { bubbles: true, cancelable: true }))
    })
    await submit()
    assert.match(container.querySelector('[role="alert"]')!.textContent!, /Unable to open/)
    fail = false
    await submit()
    assert.equal(container.querySelector('form'), null)
    assert.match(container.textContent!, /Validated at v4/)
    const posted = calls.filter((call) => call.method === 'POST').at(-1)!
    assert.equal(posted.url, '/api/sql/tables?connection_id=session-a')
    assert.deepEqual(posted.body, { name: 'recordings', uri: 's3://bucket/base.lance' })
    await act(async () => button('Remove').click())
    assert.equal(container.querySelector('.sql-table-entry'), null)
    assert.ok(calls.some((call) => call.method === 'DELETE' && call.url === '/api/sql/tables/recordings?connection_id=session-a'))
  } finally {
    await act(async () => root.unmount())
    container.remove()
    globalThis.fetch = previousFetch
  }
})
