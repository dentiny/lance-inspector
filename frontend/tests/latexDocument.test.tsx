import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import React, { act } from 'react'
import { createRoot } from 'react-dom/client'
import { JSDOM } from 'jsdom'
import { renderLatexDocument } from '../src/latexDocument'
import { LatexContent } from '../src/components/LatexContent'
import { isLatexDocument } from '../src/latexDetection'
import { ContentPreview } from '../src/components/ContentPreview'
import { MAX_STRUCTURED_CHARACTERS } from '../src/contentPreview'

const paper = readFileSync(new URL('./fixtures/paper.tex', import.meta.url), 'utf8')
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'https://viewer.example' })
Object.assign(globalThis, { window: dom.window, document: dom.window.document, localStorage: dom.window.localStorage, IS_REACT_ACT_ENVIRONMENT: true })

// Exercise the component lifecycle with the same parser used by the real worker.
class PreviewWorker {
  static terminated = 0
  onmessage?: (event: { data: unknown }) => void
  onerror?: () => void
  stopped = false
  postMessage(source: string) {
    queueMicrotask(() => {
      if (this.stopped) return
      try { this.onmessage?.({ data: { result: renderLatexDocument(source) } }) }
      catch { this.onmessage?.({ data: { error: 'This LaTeX document could not be parsed. Use Text or Copy source to inspect it.' } }) }
    })
  }
  terminate() { this.stopped = true; PreviewWorker.terminated++ }
}
Object.assign(globalThis, { Worker: PreviewWorker })

test('detects documents and structured fragments without changing math or commented examples', () => {
  assert.equal(isLatexDocument(paper), true)
  assert.equal(isLatexDocument(String.raw`\section{Intro}A paragraph.`), true)
  assert.equal(isLatexDocument(String.raw`\begin{itemize}\item A\end{itemize}`), true)
  assert.equal(isLatexDocument(String.raw`\frac{1}{2}`), false)
  assert.equal(isLatexDocument(String.raw`Text with $x^2$ and \(y\).`), false)
  assert.equal(isLatexDocument('% \\documentclass{article}\n$x$'), false)
})

test('renders a paper with an unavailable conference style as a structured document', () => {
  const { html } = renderLatexDocument(paper)
  const parsed = new JSDOM(html).window.document
  assert.equal(parsed.querySelector('header h1')?.textContent, 'Lance Inspector: Structured Document Preview')
  assert.match(parsed.querySelector('.latex-author')!.textContent!, /Alex Example/)
  assert.equal(parsed.querySelectorAll('.latex-author br').length, 2)
  assert.equal(parsed.querySelector('.latex-abstract h2')?.textContent, 'Abstract')
  assert.equal(parsed.querySelector('h3')?.textContent, 'Introduction')
  assert.equal(parsed.querySelector('h4')?.textContent, 'A decision rule')
  assert.equal(parsed.querySelector('b')?.textContent, 'formatted prose')
  assert.equal(parsed.querySelectorAll('ul li').length, 2)
  assert.equal(parsed.querySelectorAll('ol li').length, 2)
  assert.equal(parsed.querySelectorAll('table tr').length, 3)
  assert.ok(parsed.querySelectorAll('.katex').length >= 6)
  assert.equal(parsed.querySelectorAll('.katex-display').length, 2)
  assert.match(parsed.body.textContent!, /50%/)
  assert.match(parsed.body.textContent!, /\[example2026\]/)
  assert.doesNotMatch(parsed.body.textContent!, /documentclass|usepackage|hypersetup|maketitle|Formatting_Instructions/)
  assert.match(parsed.body.textContent!, /Image unavailable: missing-figure.pdf/)
  assert.match(parsed.body.textContent!, /Included file unavailable: missing-appendix.tex/)
  assert.match(parsed.body.textContent!, /Bibliography file unavailable: missing-references/)
})

test('document preview sanitizes HTML and URLs and never loads external resources', () => {
  const source = String.raw`\documentclass{article}\begin{document}
<script>alert(1)</script><img src="https://evil/image" onerror="alert(1)">
\href{javascript:alert(1)}{bad link}
\includegraphics{https://evil/image.png}
\input{/etc/passwd}
$\href{javascript:alert(1)}{math link}$
\begin{verbatim}<script>literal code</script>\end{verbatim}
\end{document}`
  const parsed = new JSDOM(renderLatexDocument(source).html).window.document
  assert.equal(parsed.querySelector('script, img, iframe, style, link, object, embed'), null)
  assert.equal(parsed.querySelector('[onerror], [onclick], a[href^="javascript:"]'), null)
  assert.match(parsed.querySelector('pre')!.textContent!, /<script>literal code<\/script>/)
  assert.match(parsed.body.textContent!, /Included file unavailable: \/etc\/passwd/)
})

test('reports unsupported commands, rejects oversized and malformed documents', () => {
  assert.ok(renderLatexDocument(String.raw`\begin{document}\unknowncommand{Visible content}\end{document}`).warnings.length > 0)
  assert.throws(() => renderLatexDocument('x'.repeat(MAX_STRUCTURED_CHARACTERS + 1)), /size limit/)
  assert.throws(() => renderLatexDocument(String.raw`\documentclass{article}\begin{document}\section{broken`))
})

test('inspector renders the document, preserves decoded source copying, and cancels workers', async () => {
  localStorage.setItem('paper-view', 'latex')
  let copied = ''
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (value: string) => { copied = value } } })
  const container = document.createElement('div'); document.body.append(container)
  const root = createRoot(container)
  const props = { value: paper, rawValue: 'hex bytes', decoded: paper, label: 'data', preference: 'paper-view', wrap: true, pretty: false, onWrap: () => {}, onPretty: () => {}, formattedJson: () => undefined }
  await act(async () => root.render(<ContentPreview {...props} />))
  assert.ok(container.querySelector('.latex-paper header h1'))
  assert.match(container.textContent!, /Custom package styling/)
  await act(async () => Array.from(container.querySelectorAll<HTMLButtonElement>('button')).find((node) => node.textContent === 'Copy source')!.click())
  assert.equal(copied, paper)
  await act(async () => Array.from(container.querySelectorAll<HTMLButtonElement>('button')).find((node) => node.textContent === 'Text')!.click())
  assert.equal(container.querySelector('pre')?.textContent, paper)
  await act(async () => root.render(<LatexContent text={String.raw`\section{New document}Updated body.`} label="new" />))
  assert.equal(container.querySelector('h3')?.textContent, 'New document')
  assert.doesNotMatch(container.textContent!, /Lance Inspector: Structured Document Preview/)
  await act(async () => root.render(<LatexContent text={String.raw`\documentclass{article}\begin{document}\section{broken`} label="broken" />))
  assert.match(container.querySelector('[role="status"]')!.textContent!, /could not be parsed/)
  await act(async () => root.unmount()); container.remove()
  assert.ok(PreviewWorker.terminated >= 3)
})
