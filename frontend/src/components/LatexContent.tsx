import { useEffect, useRef } from 'react'
import katex from 'katex'
import renderMathInElement from 'katex/contrib/auto-render'
import { mathOptions } from '../mathOptions'
import { isLatexDocument } from '../latexDetection'
import { LatexDocument } from './LatexDocument'

const delimiters = [
  { left: '$$', right: '$$', display: true },
  { left: '\\[', right: '\\]', display: true },
  { left: '\\(', right: '\\)', display: false },
  { left: '$', right: '$', display: false },
]

export function LatexContent({ text, label }: { text: string; label: string }) {
  return isLatexDocument(text) ? <LatexDocument text={text} label={label} /> : <LatexMath text={text} label={label} />
}

function LatexMath({ text, label }: { text: string; label: string }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const element = ref.current!
    // Assign text rather than HTML so prose and failed formulas remain inert.
    element.textContent = text
    if (delimiters.some(({ left }) => text.includes(left))) {
      renderMathInElement(element, { ...mathOptions, delimiters, throwOnError: false })
    } else {
      katex.render(text, element, { ...mathOptions, displayMode: true, throwOnError: false })
    }
  }, [text])
  return <div ref={ref} className="content-latex" aria-label={`${label} rendered as LaTeX`} />
}
