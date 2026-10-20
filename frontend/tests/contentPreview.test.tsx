import assert from 'node:assert/strict'
import { test } from 'node:test'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'
import { createRoot } from 'react-dom/client'
import { fieldLabel, MAX_STRUCTURED_CHARACTERS, preferenceKey, sourceText, structuredValue } from '../src/contentPreview'
import { feedToHtml, previewDocument } from '../src/contentPreviewHtml'
import { ContentPreview } from '../src/components/ContentPreview'
import { JsonFieldTree } from '../src/components/JsonFieldTree'

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'https://viewer.example' })
Object.assign(globalThis, { window: dom.window, document: dom.window.document, DOMParser: dom.window.DOMParser, localStorage: dom.window.localStorage, IS_REACT_ACT_ENVIRONMENT: true })

test('recognizes structured cells and complete JSONL while preserving source strings', () => {
  const source = '{"steps":[{"message":"# Hello\\n\\n**World**"}]}'
  const value = structuredValue(source) as { steps: { message: string }[] }
  assert.equal(value.steps[0].message, '# Hello\n\n**World**')
  assert.equal(sourceText(source), source)
  assert.deepEqual(structuredValue('{"html":"<h1>A</h1>"}\n{"html":"<h1>B</h1>"}\n'), [{ html: '<h1>A</h1>' }, { html: '<h1>B</h1>' }])
  for (const value of ['plain text', '# Markdown', '<div>HTML</div>', '{broken}', '{"x":1}\n{"partial":', '"text"', 'null']) assert.equal(structuredValue(value), undefined)
  assert.equal(structuredValue(' '.repeat(MAX_STRUCTURED_CHARACTERS) + '{}'), undefined)
})

test('field paths distinguish unusual object keys and share preferences across list indices', () => {
  assert.equal(fieldLabel('raw_json', ['steps', 3, 'a.b', 'text']), 'raw_json.steps[3]["a.b"].text')
  assert.equal(preferenceKey('dataset', 'steps', [0, 'message']), preferenceKey('dataset', 'steps', [10, 'message']))
  assert.notEqual(preferenceKey('dataset', 'steps', ['0', 'message']), preferenceKey('dataset', 'steps', [0, 'message']))
  assert.notEqual(preferenceKey('one', 'text', []), preferenceKey('two', 'text', []))
})

test('HTML preserves layout while disabling scripts, remote resources, and navigation', () => {
  const html = previewDocument('<!doctype html><html><head><base href="https://evil/"><meta http-equiv="refresh" content="0;url=https://evil/"><style>h1 {color:red}</style></head><body><h1 onclick="alert(1)">Hello</h1><img src="images/a.png" srcset="https://evil/a 2x"><iframe src="https://evil/"></iframe><script>alert(1)</script><a href="https://evil/">link</a></body></html>')
  const parsed = new dom.window.DOMParser().parseFromString(html, 'text/html')
  assert.equal(parsed.querySelector('h1')?.textContent, 'Hello')
  assert.equal(parsed.querySelector('h1')?.getAttribute('onclick'), null)
  assert.equal(parsed.querySelector('script, iframe, base, meta[http-equiv="refresh"]'), null)
  assert.equal(parsed.querySelector('a')?.getAttribute('href'), null)
  assert.equal(parsed.querySelector('img')?.getAttribute('srcset'), null)
  assert.equal(parsed.querySelector('img')!.getAttribute('src'), 'images/a.png')
  assert.match(parsed.head.firstElementChild!.getAttribute('content')!, /img-src data:;/)
  assert.doesNotMatch(html, /content-resource/)
  assert.equal(parsed.head.firstElementChild?.getAttribute('http-equiv'), 'Content-Security-Policy')
  assert.match(parsed.head.firstElementChild!.getAttribute('content')!, /default-src 'none'/)
  assert.equal(parsed.querySelector('style')?.textContent, 'h1 {color:red}')
})

test('Atom/RSS rendered HTML remains supported', () => {
  const feed = feedToHtml('<rss><channel><title>Feed</title><item><title>Entry</title><description><![CDATA[<p>Content</p>]]></description></item></channel></rss>')
  assert.equal(feed?.entryCount, 1)
  assert.match(feed!.html, /<p>Content<\/p>/)
  assert.equal(feedToHtml('<h1>Not a feed</h1>'), undefined)
})

test('nested JSON selection reaches the exact string and pages large arrays', async () => {
  const container = document.createElement('div'); document.body.append(container)
  const root = createRoot(container)
  let selected: unknown
  let path: unknown
  await act(async () => root.render(<JsonFieldTree value='{"trajectory":{"message":"# Hello"}}' label="raw_json" selectedPath={[]} onSelect={(nextPath, nextValue) => { path = nextPath; selected = nextValue }} />))
  const expand = container.querySelector<HTMLButtonElement>('[aria-label="Expand trajectory"]')!
  await act(async () => expand.click())
  const button = Array.from(container.querySelectorAll<HTMLButtonElement>('.json-field-value')).find((node) => node.textContent?.includes('# Hello'))!
  await act(async () => button.click())
  assert.equal(selected, '# Hello'); assert.deepEqual(path, ['trajectory', 'message'])
  await act(async () => root.render(<JsonFieldTree value={Array.from({length: 125}, (_, i) => `row ${i}`)} label="rows" selectedPath={[]} onSelect={() => {}} />))
  assert.equal(container.querySelectorAll('.json-field-leaf').length, 50)
  await act(async () => container.querySelector<HTMLButtonElement>('.json-field-more')!.click())
  assert.equal(container.querySelectorAll('.json-field-leaf').length, 100)
  await act(async () => root.unmount()); container.remove()
})

test('preview switches Markdown, HTML, and source without changing or copying rendered markup', async () => {
  localStorage.clear()
  let copied = ''
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { copied = text } } })
  const container = document.createElement('div'); document.body.append(container)
  const root = createRoot(container)
  const value = '# Heading\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n<script>alert(1)</script>'
  const preference = preferenceKey('dataset', 'raw_json', ['message'])
  await act(async () => root.render(<ContentPreview value={value} rawValue={value} label="message" preference={preference} wrap pretty onWrap={() => {}} onPretty={() => {}} formattedJson={() => undefined} />))
  const click = async (name: string) => { await act(async () => Array.from(container.querySelectorAll<HTMLButtonElement>('button')).find((node) => node.textContent === name)!.click()) }
  assert.equal(container.querySelector('pre')?.textContent, value)
  await click('Markdown')
  assert.equal(container.querySelector('h1')?.textContent, 'Heading')
  assert.equal(container.querySelectorAll('tbody tr').length, 1)
  assert.equal(container.querySelector('script'), null)
  assert.equal(localStorage.getItem(preference), 'markdown')
  await click('Copy source'); assert.equal(copied, value)
  await click('HTML')
  assert.equal(container.querySelector('iframe')?.getAttribute('sandbox'), '')
  assert.match(container.querySelector('iframe')!.srcdoc, /Content-Security-Policy/)
  await click('Text'); assert.equal(container.querySelector('pre')?.textContent, value)
  await act(async () => root.unmount()); container.remove()
})


test('remembers rendering preference and bounds rich rendering of large values', async () => {
  const preference = preferenceKey('dataset', 'large', [])
  localStorage.setItem(preference, 'html')
  const container = document.createElement('div'); document.body.append(container)
  const root = createRoot(container)
  const props = { label: 'large', preference, wrap: true, pretty: true, onWrap: () => {}, onPretty: () => {}, formattedJson: () => undefined }
  await act(async () => root.render(<ContentPreview {...props} value="<h1>Remembered</h1>" rawValue="<h1>Remembered</h1>" />))
  assert.match(container.querySelector('iframe')!.srcdoc, /Remembered/)
  const large = 'x'.repeat(MAX_STRUCTURED_CHARACTERS + 1)
  await act(async () => root.render(<ContentPreview {...props} value={large} rawValue={large} />))
  assert.equal(container.querySelector('iframe'), null)
  assert.match(container.textContent!, /too large|exceeds/i)
  await act(async () => Array.from(container.querySelectorAll<HTMLButtonElement>('button')).find((node) => node.textContent === 'Text')!.click())
  assert.equal(container.querySelector('pre')!.textContent, large)
  await act(async () => root.unmount()); container.remove()
})
