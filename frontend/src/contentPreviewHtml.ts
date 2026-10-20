const escapeHtml = (text: string) => text
  .replace(/&/g, '&amp;')
  .replace(/</g, '&lt;')
  .replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;')

const childText = (element: Element, name: string) => {
  const child = Array.from(element.children).find((candidate) => candidate.localName === name)
  return child?.textContent?.trim() ?? ''
}

const feedStyle = `<style>
  body { margin: 0; padding: 20px; color: #1f2328; font: 14px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
  header { margin-bottom: 18px; padding-bottom: 12px; border-bottom: 1px solid #d0d7de; }
  header h1 { margin: 0 0 4px; font-size: 18px; }
  header p, .meta { margin: 0; color: #656d76; font-size: 12px; }
  article { padding: 14px 0; border-bottom: 1px solid #eaeef2; }
  article h2 { margin: 0 0 4px; font-size: 15px; }
  .content { margin-top: 8px; overflow-wrap: anywhere; }
  .content p { margin: 0 0 8px; }
</style>`

// Atom/RSS documents carry entry bodies as escaped HTML, so they must be unpacked before rendering.
export const feedToHtml = (source: string) => {
  const trimmed = source.trimStart()
  if (!/^(<\?xml[^>]*>\s*)?<(feed|rss)[\s>]/.test(trimmed)) return undefined
  const parsed = new DOMParser().parseFromString(trimmed, 'application/xml')
  if (parsed.getElementsByTagName('parsererror').length > 0) return undefined
  const root = parsed.documentElement
  const isAtom = root.localName === 'feed'
  const channel = isAtom ? root : Array.from(root.children).find((child) => child.localName === 'channel')
  if (!channel) return undefined
  const entries = Array.from(channel.children).filter((child) => child.localName === (isAtom ? 'entry' : 'item'))
  const articles = entries.map((entry) => {
    const author = isAtom
      ? childText(Array.from(entry.children).find((child) => child.localName === 'author') ?? entry, 'name')
      : childText(entry, 'creator') || childText(entry, 'author')
    const date = isAtom ? childText(entry, 'updated') || childText(entry, 'published') : childText(entry, 'pubDate')
    const contentElement = Array.from(entry.children).find((child) => child.localName === (isAtom ? 'content' : 'encoded'))
      ?? Array.from(entry.children).find((child) => child.localName === (isAtom ? 'summary' : 'description'))
    const contentType = contentElement?.getAttribute('type') ?? 'html'
    const body = !contentElement
      ? ''
      : contentType === 'xhtml'
        ? contentElement.innerHTML
        : contentType === 'text'
          ? `<p>${escapeHtml(contentElement.textContent ?? '')}</p>`
          : contentElement.textContent ?? ''
    const meta = [author, date].filter(Boolean).map(escapeHtml).join(' · ')
    const title = childText(entry, 'title')
    return `<article>${title ? `<h2>${escapeHtml(title)}</h2>` : ''}${meta ? `<p class="meta">${meta}</p>` : ''}<div class="content">${body}</div></article>`
  })
  const feedTitle = childText(channel, 'title')
  const feedSubtitle = childText(channel, isAtom ? 'subtitle' : 'description')
  return {
    entryCount: entries.length,
    html: `${feedStyle}<header>${feedTitle ? `<h1>${escapeHtml(feedTitle)}</h1>` : ''}${feedSubtitle ? `<p>${escapeHtml(feedSubtitle)}</p>` : ''}</header>${articles.join('')}`,
  }
}


// Keep the policy ahead of document content, including malformed/full HTML documents.
export function previewDocument(source: string): string {
  const document = new DOMParser().parseFromString(source, 'text/html')
  document.querySelectorAll('script, iframe, object, embed, base, link, meta[http-equiv]').forEach((node) => node.remove())
  document.querySelectorAll('*').forEach((node) => {
    for (const attribute of Array.from(node.attributes)) {
      if (/^on/i.test(attribute.name) || ['srcset', 'formaction', 'action', 'ping'].includes(attribute.name)) node.removeAttribute(attribute.name)
    }
  })
  // Prevent links from navigating the preview away from the source document.
  document.querySelectorAll('a, area').forEach((link) => { link.removeAttribute('href'); link.removeAttribute('target') })
  const policy = document.createElement('meta')
  policy.httpEquiv = 'Content-Security-Policy'
  policy.content = `default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; form-action 'none'; base-uri 'none'`
  document.head.prepend(policy)
  return '<!doctype html>' + document.documentElement.outerHTML
}
