import { renderLatexDocument } from './latexDocument'

self.onmessage = ({ data }: MessageEvent<string>) => {
  try { self.postMessage({ result: renderLatexDocument(data) }) }
  catch { self.postMessage({ error: 'This LaTeX document could not be parsed. Use Text or Copy source to inspect it.' }) }
}
