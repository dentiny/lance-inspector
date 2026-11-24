import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import remarkMath from 'remark-math'
import rehypeKatex from 'rehype-katex'
import { mathOptions } from '../mathOptions'

export function MarkdownContent({ text, label }: { text: string; label: string }) {
  return <div className="content-markdown" aria-label={`${label} rendered as Markdown`}>
    <ReactMarkdown remarkPlugins={[remarkGfm, remarkMath]} rehypePlugins={[[rehypeKatex, mathOptions]]}
      components={{ a: ({ children, href }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a> }}
    >{text}</ReactMarkdown>
  </div>
}
