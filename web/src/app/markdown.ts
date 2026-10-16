import DOMPurify from 'dompurify'
import MarkdownIt from 'markdown-it'

const md = new MarkdownIt({ html: false, linkify: true, breaks: true })

const style = new CSSStyleSheet()
style.replaceSync(`
  :host { display: block; overflow-wrap: anywhere; }
  .body > :first-child { margin-top: 0; }
  .body > :last-child { margin-bottom: 0; }
  p, ul, ol, pre, blockquote, table { margin: 0 0 0.75em; }
  pre { padding: 10px 12px; border-radius: 6px; background: var(--panel); overflow-x: auto; font: 12.5px/1.5 var(--mono); }
  code { font-family: var(--mono); font-size: 0.92em; }
  h1, h2, h3, h4 { margin: 1em 0 0.5em; font-size: 1.05em; font-weight: 600; }
  a { color: inherit; }
  blockquote { padding-left: 12px; border-left: 2px solid var(--line); color: var(--muted); }
  table { border-collapse: collapse; }
  th, td { padding: 4px 8px; border: 1px solid var(--line); }
`)

/**
 * `<yeet-markdown source="…">`. Mün owns an element's light-DOM children and
 * cannot set innerHTML, so the rendered markdown lives in a shadow root.
 */
class YeetMarkdown extends HTMLElement {
  static observedAttributes = ['source']
  private readonly body: HTMLDivElement
  private text: string | null = null

  constructor() {
    super()
    const root = this.attachShadow({ mode: 'open' })
    root.adoptedStyleSheets = [style]
    this.body = document.createElement('div')
    this.body.className = 'body'
    root.append(this.body)
  }

  get source() { return this.text ?? '' }
  set source(value: string) {
    if (value === this.text) return
    this.text = value ?? ''
    this.body.innerHTML = DOMPurify.sanitize(md.render(this.text))
  }

  attributeChangedCallback(_name: string, _old: string | null, value: string | null) { this.source = value ?? '' }
}

if (!customElements.get('yeet-markdown')) customElements.define('yeet-markdown', YeetMarkdown)
