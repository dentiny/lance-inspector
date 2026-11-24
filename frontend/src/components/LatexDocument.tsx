import { useEffect, useState } from 'react'
import type { LatexDocumentResult } from '../latexDocument'

export function LatexDocument({ text, label }: { text: string; label: string }) {
  const [preview, setPreview] = useState<{ source: string; result?: LatexDocumentResult; error?: string }>()
  useEffect(() => {
    const worker = new Worker(new URL('../latexDocument.worker.ts', import.meta.url), { type: 'module' })
    const timer = window.setTimeout(() => {
      worker.terminate()
      setPreview({ source: text, error: 'LaTeX preview timed out. Use Text or Copy source to inspect it.' })
    }, 5000)
    worker.onmessage = ({ data }: MessageEvent<{ result?: LatexDocumentResult; error?: string }>) => {
      window.clearTimeout(timer)
      setPreview({ source: text, ...data })
      worker.terminate()
    }
    worker.onerror = () => {
      window.clearTimeout(timer)
      setPreview({ source: text, error: 'LaTeX preview failed. Use Text or Copy source to inspect it.' })
      worker.terminate()
    }
    worker.postMessage(text)
    return () => { window.clearTimeout(timer); worker.terminate() }
  }, [text])
  const current = preview?.source === text ? preview : undefined
  return <div className="content-latex-document" aria-label={`${label} rendered as LaTeX document`}>
    <p className="preview-note">Document preview. Custom package styling, external files, and bibliography resolution are unavailable; layout may differ from a compiled PDF.</p>
    {current?.error ? <p role="status" className="preview-note">{current.error}</p>
      : current?.result ? <>
        {current.result.warnings.length > 0 && <details className="preview-note"><summary>Some LaTeX commands could not be rendered</summary><ul>{current.result.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul></details>}
        <article className="latex-paper" dangerouslySetInnerHTML={{ __html: current.result.html }} />
      </> : <p role="status" className="preview-note">Rendering LaTeX document…</p>}
  </div>
}
