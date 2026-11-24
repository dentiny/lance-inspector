import type * as Ast from '@unified-latex/unified-latex-types'
import type { Element, Root } from 'hast'
import { getParser } from '@unified-latex/unified-latex-util-parse'
import { getArgsContent } from '@unified-latex/unified-latex-util-arguments'
import { printRaw } from '@unified-latex/unified-latex-util-print-raw'
import { htmlLike } from '@unified-latex/unified-latex-util-html-like'
import { unifiedLatexToHast } from '@unified-latex/unified-latex-to-hast'
import { unified } from 'unified'
import rehypeSanitize from 'rehype-sanitize'
import katex from 'katex'
import { fromHtml } from 'hast-util-from-html'
import rehypeStringify from 'rehype-stringify'
import { MAX_STRUCTURED_CHARACTERS } from './contentPreview'
import { mathOptions } from './mathOptions'

export type LatexDocumentResult = { html: string; warnings: string[] }

const string = (content: string): Ast.String => ({ type: 'string', content })
const lastArg = (node: Ast.Macro | Ast.Environment) => getArgsContent(node).at(-1) ?? []

// Parse syntax, never execute TeX or load packages, images, or included files.
export function renderLatexDocument(source: string): LatexDocumentResult {
  if (source.length > MAX_STRUCTURED_CHARACTERS) throw new Error('Document exceeds the preview size limit.')
  const parser = getParser({ macros: {
    input: { signature: 'm' }, include: { signature: 'm' },
    citep: { signature: 's o o m' }, citet: { signature: 's o o m' },
    bibliography: { signature: 'm' }, bibliographystyle: { signature: 'm' },
  } })
  const tree = parser.runSync(parser.parse(source))
  const metadata = new Map<string, Ast.Node[]>()
  const document = tree.content.find((node): node is Ast.Environment => node.type === 'environment' && node.env === 'document')
  if (!document && /\\begin\s*\{document\}/.test(source.replace(/(?<!\\)%[^\r\n]*/g, ''))) throw new Error('Incomplete document environment.')
  for (const node of [...tree.content, ...(document?.content ?? [])]) {
    if (node.type === 'macro' && ['title', 'author', 'date'].includes(node.content)) metadata.set(node.content, lastArg(node))
  }
  // Drop the preamble after collecting title metadata. A missing custom .sty file
  // must not prevent the remaining article from being read.
  if (document) tree.content = document.content
  const titleBlock = () => htmlLike({ tag: 'header', content: [
    htmlLike({ tag: 'h1', content: structuredClone(metadata.get('title') ?? []) }),
    htmlLike({ tag: 'div', attributes: { className: 'latex-author' }, content: structuredClone(metadata.get('author') ?? []) }),
    htmlLike({ tag: 'div', attributes: { className: 'latex-date' }, content: structuredClone(metadata.get('date') ?? []) }),
  ] })
  const insertTitle = (nodes: Ast.Node[]): Ast.Node[] => nodes.flatMap((node): Ast.Node | Ast.Node[] => {
    if (node.type === 'macro' && node.content === 'maketitle') return titleBlock()
    if (node.type === 'macro' && ['toprule', 'midrule', 'bottomrule', 'hline', 'centering'].includes(node.content)) return []
    if ('content' in node && Array.isArray(node.content)) node.content = insertTitle(node.content)
    return node
  })
  tree.content = insertTitle(tree.content)
  const replacements: Record<string, (node: Ast.Macro) => Ast.Node> = {
    title: () => string(''), author: () => string(''), date: () => string(''),
    label: () => string(''),
    caption: (node) => htmlLike({ tag: 'figcaption', content: lastArg(node) }),
    par: () => htmlLike({ tag: 'br' }),
    quad: () => string('\u2003'), qquad: () => string('\u2003\u2003'),
    and: () => htmlLike({ tag: 'br' }),
    LaTeX: () => string('LaTeX'), TeX: () => string('TeX'),
    includegraphics: (node) => htmlLike({ tag: 'aside', content: [string(`[Image unavailable: ${printRaw(lastArg(node))}]`)] }),
    input: (node) => htmlLike({ tag: 'aside', content: [string(`[Included file unavailable: ${printRaw(lastArg(node))}]`)] }),
    include: (node) => htmlLike({ tag: 'aside', content: [string(`[Included file unavailable: ${printRaw(lastArg(node))}]`)] }),
    bibliography: (node) => htmlLike({ tag: 'aside', content: [string(`[Bibliography file unavailable: ${printRaw(lastArg(node))}]`)] }),
    bibliographystyle: () => string(''),
  }
  for (const name of ['cite', 'citep', 'citet', 'ref', 'eqref', 'autoref']) {
    replacements[name] = (node) => string(`[${printRaw(lastArg(node))}]`)
  }
  const converted = getParser()().use(unifiedLatexToHast, {
    macroReplacements: replacements,
    environmentReplacements: {
      table: (node) => htmlLike({ tag: 'figure', content: node.content }),
      figure: (node) => htmlLike({ tag: 'figure', content: node.content }),
      abstract: (node) => htmlLike({ tag: 'section', attributes: { className: 'latex-abstract' }, content: [htmlLike({ tag: 'h2', content: [string('Abstract')] }), ...node.content] }),
    },
  })
  const file = { messages: [] as { message: string }[] }
  const hast = converted.runSync(tree, file) as unknown as Root
  // Remove the empty trailing table row emitted for a final LaTeX linebreak.
  const textContent = (node: Root | Element): string => node.children.map((child) => child.type === 'text' ? child.value : child.type === 'element' ? textContent(child) : '').join('')
  const trimTables = (node: Root | Element) => {
    if (node.type === 'element' && node.tagName === 'tbody') {
      const last = node.children.at(-1)
      if (last?.type === 'element' && last.tagName === 'tr' && !textContent(last).trim()) node.children.pop()
    }
    for (const child of node.children) if (child.type === 'element') trimTables(child)
  }
  trimTables(hast)
  const html = unified().use(rehypeSanitize, {
    tagNames: ['figure', 'figcaption', 'header', 'section', 'aside', 'div', 'span', 'p', 'br', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'b', 'strong', 'i', 'em', 'u', 'del', 'sup', 'sub', 'ul', 'ol', 'li', 'dl', 'dt', 'dd', 'blockquote', 'pre', 'code', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'hr', 'a'],
    attributes: {
      '*': [['className', 'inline-math', 'display-math', 'latex-author', 'latex-date', 'latex-abstract', 'texttt']],
      a: ['href'], th: ['colSpan', 'rowSpan'], td: ['colSpan', 'rowSpan'],
    },
    protocols: { href: ['https', 'http', 'mailto'] },
  }).use(rehypeStringify)
  const result = html.runSync(hast)
  // Sanitize dataset-derived HTML before adding KaTeX's non-interactive output.
  // Use parse5 rather than DOMParser: web workers have no browser DOM.
  const renderMath = (node: Root | Element) => {
    for (const child of node.children) {
      if (child.type !== 'element') continue
      const classes = child.properties.className
      if (Array.isArray(classes) && (classes.includes('inline-math') || classes.includes('display-math'))) {
        const formula = katex.renderToString(textContent(child), { ...mathOptions, displayMode: classes.includes('display-math'), throwOnError: false })
        child.children = fromHtml(formula, { fragment: true }).children.filter((item) => item.type !== 'doctype')
      } else renderMath(child)
    }
  }
  renderMath(result)
  return { html: html.stringify(result), warnings: [...new Set(file.messages.map(({ message }) => message.slice(0, 200)))].slice(0, 20) }
}
