import { useEffect, useMemo, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Check, Copy, WrapText } from 'lucide-react'
import { MAX_STRUCTURED_CHARACTERS, sourceText, type ContentView } from '../contentPreview'
import { feedToHtml, previewDocument } from '../contentPreviewHtml'

function savedView(key: string): ContentView {
  try {
    const view = localStorage.getItem(key)
    if (view === 'text' || view === 'markdown' || view === 'html') return view
  } catch { /* Storage can be unavailable in private/embedded contexts. */ }
  return 'raw'
}

export function ContentPreview({ value, rawValue, decoded, label, preference, wrap, pretty, onWrap, onPretty, formattedJson }: {
  value: unknown
  rawValue: unknown
  decoded?: string
  label: string
  preference: string
  wrap: boolean
  pretty: boolean
  onWrap: () => void
  onPretty: () => void
  formattedJson: (value: unknown) => string | undefined
}) {
  const [view, setView] = useState<ContentView>(() => typeof value === 'string' ? savedView(preference) : 'raw')
  const [copied, setCopied] = useState(false)
  const [copyError, setCopyError] = useState('')
  const text = sourceText(view === 'raw' ? rawValue : value)
  const formatted = useMemo(() => formattedJson(view === 'raw' ? rawValue : value), [view, rawValue, value, formattedJson])
  const rendered = view === 'markdown' || view === 'html'
  const previewTooLarge = text.length > MAX_STRUCTURED_CHARACTERS
  const feed = useMemo(() => view === 'html' && !previewTooLarge && typeof value === 'string' ? feedToHtml(value) : undefined, [view, value, previewTooLarge])
  const html = useMemo(() => view === 'html' && !previewTooLarge ? previewDocument(feed?.html ?? text) : '', [view, feed, text, previewTooLarge])
  const byteCount = useMemo(() => new TextEncoder().encode(text).length, [text])

  useEffect(() => { setCopied(false); setCopyError('') }, [value, view])
  useEffect(() => {
    if (!copied) return
    const timer = window.setTimeout(() => setCopied(false), 1500)
    return () => window.clearTimeout(timer)
  }, [copied])

  const choose = (next: ContentView) => {
    setView(next)
    try { localStorage.setItem(preference, next) } catch { /* Preview still works without persistence. */ }
  }
  const copy = async () => {
    try { await navigator.clipboard.writeText(text); setCopied(true); setCopyError('') }
    catch { setCopyError('Copy unavailable; select and copy the source text.') }
  }

  return <>
    <div className="value-inspector-meta">
      <span>{text.length.toLocaleString()} characters</span><span>{byteCount.toLocaleString()} bytes</span>
      {decoded !== undefined && <span>binary · UTF-8</span>}
      {feed && <span>Atom/RSS · {feed.entryCount} entries</span>}
    </div>
    <div className="value-inspector-toolbar">
      <div className="value-inspector-views" role="group" aria-label="View as">
        <button type="button" aria-pressed={view === 'raw'} className={view === 'raw' ? 'active' : ''} onClick={() => choose('raw')}>{decoded !== undefined ? 'Hex' : 'Text'}</button>
        {decoded !== undefined && <button type="button" className={view === 'text' ? 'active' : ''} onClick={() => choose('text')}>Text</button>}
        {typeof value === 'string' && <>
          <button type="button" aria-pressed={view === 'markdown'} className={view === 'markdown' ? 'active' : ''} onClick={() => choose('markdown')}>Markdown</button>
          <button type="button" aria-pressed={view === 'html'} className={view === 'html' ? 'active' : ''} onClick={() => choose('html')}>HTML</button>
        </>}
      </div>
      {!rendered && formatted && <button type="button" className={pretty ? 'active' : ''} onClick={onPretty}>{'{ }'} {pretty ? 'Pretty JSON' : 'Raw JSON'}</button>}
      {!rendered && <button type="button" className={wrap ? 'active' : ''} onClick={onWrap}><WrapText size={14} />Wrap</button>}
      <button type="button" onClick={() => void copy()}>{copied ? <Check size={14} /> : <Copy size={14} />}{copied ? 'Copied' : 'Copy source'}</button>
    </div>
    {copyError && <p role="status" className="preview-note">{copyError}</p>}
    {rendered && previewTooLarge ? <p className="preview-note" role="status">Content is too large for rich preview. Use Text or Copy source to inspect the complete value.</p> : view === 'markdown' ? <div className="content-markdown" aria-label={`${label} rendered as Markdown`}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}
        components={{ a: ({ children, href }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a> }}
      >{text}</ReactMarkdown>
    </div> : view === 'html' ? <iframe className="value-inspector-html" title={`${label} rendered as HTML`} sandbox="" referrerPolicy="no-referrer" srcDoc={html} />
      : <pre className={wrap ? 'wrap' : ''}>{pretty && formatted ? formatted : text}</pre>}
  </>
}
