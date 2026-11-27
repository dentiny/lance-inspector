import assert from 'node:assert/strict'
import { test } from 'node:test'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/' })
// Keep FileTree's viewport-fill behavior separate from explicit pagination checks.
Object.defineProperty(dom.window.HTMLElement.prototype, 'scrollHeight', { value: 1000 })
Object.assign(globalThis, {
  window: dom.window, document: dom.window.document,
  IS_REACT_ACT_ENVIRONMENT: true,
  ResizeObserver: class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
})
// React must detect the DOM before initializing its input event handling.
const { createRoot } = await import('react-dom/client')
const { default: App } = await import('../src/App')

test('listing is lazy, recoverable, and isolated from a replacement connection', async () => {
  const previousFetch = globalThis.fetch
  const listings: { url: string; signal?: AbortSignal | null; finish: (response: Response) => void }[] = []
  let connections = 0
  globalThis.fetch = async (input, init) => {
    const url = String(input)
    if (url === '/api/dataset/references') return Response.json({
      discovery_id: 'root', uri: 'memory://opening', tags: [],
      branches: [{ name: 'main', parent_branch: null, parent_version: null,
        versions: [{ version: 1, total_rows: 0, tags: [] }] }],
    })
    if (url === '/api/dataset/connect') return Response.json({
      connection_id: `session-${++connections}`, dataset: {
        uri: 'memory://opening', reference: '1', version: 1, branch: 'main', rows: 0,
        schema: [], indices: [], fragments: [],
        manifest: { config: {}, table_metadata: {}, base_paths: {}, timestamp_nanos: '0' },
      },
    })
    if (url.startsWith('/api/files?')) return new Promise((finish) => {
      listings.push({ url, signal: init?.signal, finish })
    })
    if (url.startsWith('/api/sql/start?')) return Response.json({ error: 'No query fixture' }, { status: 500 })
    throw new Error(`Unexpected request: ${url}`)
  }
  const container = document.createElement('div')
  document.body.append(container)
  const root = createRoot(container)
  const button = (label: string) => Array.from(container.querySelectorAll('button')).find((item) => item.textContent === label)!
  const selectSnapshot = async () => {
    await act(async () => container.querySelector<HTMLButtonElement>('.lineage-node')!.click())
  }
  const scroll = async () => {
    await act(async () => {
      const tree = container.querySelector<HTMLElement>('.file-tree')!
      tree.scrollTop = 1000
      tree.dispatchEvent(new dom.window.Event('scroll'))
    })
  }
  try {
    await act(async () => root.render(<App />))
    // Supply a location through the native input, then submit the discovery form.
    const input = container.querySelector('input')!
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value')!.set!
    await act(async () => {
      setter.call(input, 'memory://opening')
      input.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
      input.dispatchEvent(new dom.window.Event('change', { bubbles: true }))
    })
    await act(async () => container.querySelector('form')!.dispatchEvent(new dom.window.Event('submit', { bubbles: true, cancelable: true })))
    await selectSnapshot()
    assert.ok(container.querySelector('.app-shell'))
    assert.equal(listings.length, 0)
    await act(async () => button('Infra').click())
    assert.equal(listings.length, 1)
    assert.ok(container.querySelector('.app-shell'))
    await act(async () => listings[0].finish(Response.json({ error: 'Storage unavailable' }, { status: 503 })))
    assert.match(container.querySelector('[role="alert"]')!.textContent!, /Storage unavailable/)
    assert.ok(container.querySelector('.app-shell'))
    await act(async () => button('Retry file listing').click())
    assert.equal(listings.length, 2)
    await act(async () => listings[1].finish(Response.json({ entries: [], next_offset: 500 })))
    assert.equal(container.querySelector('[role="alert"]'), null)
    await act(async () => button('User').click())
    await act(async () => button('Infra').click())
    assert.equal(listings.length, 2)
    await scroll()
    assert.match(listings[2].url, /offset=500/)
    await act(async () => listings[2].finish(Response.json({ error: 'Page unavailable' }, { status: 503 })))
    assert.match(container.querySelector('[role="alert"]')!.textContent!, /Page unavailable/)
    await scroll()
    assert.equal(listings.length, 3)
    await act(async () => button('Retry file listing').click())
    assert.match(listings[3].url, /offset=500/)
    assert.equal(button('Retry file listing').disabled, true)
    await act(async () => container.querySelector<HTMLButtonElement>('.reference-switch')!.click())
    await selectSnapshot()
    assert.equal(listings[3].signal?.aborted, true)
    assert.equal(listings.length, 5)
    assert.match(listings[4].url, /connection_id=session-2/)
    await act(async () => listings[3].finish(Response.json({ error: 'Stale failure' }, { status: 503 })))
    assert.equal(container.querySelector('[role="alert"]'), null)
    await act(async () => listings[4].finish(Response.json({ entries: [], next_offset: null })))
    assert.ok(container.querySelector('.app-shell'))
    assert.equal(container.querySelector('[role="alert"]'), null)
  } finally {
    await act(async () => root.unmount())
    container.remove()
    globalThis.fetch = previousFetch
  }
})
