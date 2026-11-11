import type { Terminal } from '@xterm/xterm'
import type { FitAddon } from '@xterm/addon-fit'
import xtermCss from '@xterm/xterm/css/xterm.css?inline'
import { remoteStore } from '@/store/remoteStore'
import type { ShellJobOutputPage, ShellJobResponse } from '@/remote/protocol'

const style = new CSSStyleSheet()
style.replaceSync(`${xtermCss}
  :host { display: block; min-width: 0; width: 100%; padding: 8px 12px; }
  * { box-sizing: border-box; }
  .shell-job-terminal { height: 220px; min-width: 0; overflow: hidden; padding: 6px; border-radius: 6px; background: #111; color-scheme: dark; }
  .shell-job-controls { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; padding: 8px 0 0; font: 12px/1.5 var(--sans); }
  button { min-height: 32px; padding: 5px 10px; border: 1px solid var(--line); border-radius: 6px; background: var(--field); color: var(--text); font: inherit; cursor: pointer; }
  button:hover { background: var(--hover); }
  button:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
  .shell-job-status { flex: 1 1 120px; padding: 4px; color: var(--muted); overflow-wrap: anywhere; }
  @media (hover: none), (pointer: coarse) { button { min-height: 44px; } }
`)

/** A viewer for an existing approved job; it never launches a command. */
class YeetShellJob extends HTMLElement {
  static observedAttributes = ['job-id']
  private readonly content: ShadowRoot
  private term: Terminal | null = null
  private fit: FitAddon | null = null
  private job = ''
  private offset = 0
  private pending: string | null = null
  private pendingInput: string | null = null
  private inputQueue: string[] = []
  private queuedInputBytes = 0
  private done = false
  private unsubscribe: (() => void) | null = null
  private unsubscribeState: (() => void) | null = null
  private observer: ResizeObserver | null = null
  private status: HTMLElement | null = null
  private wasConnected = false
  private generation = 0
  private retryTimer: number | null = null

  constructor() {
    super()
    // Mün owns light-DOM children and may reconcile them on any state update.
    // Like yeet-markdown, the imperative renderer must own a shadow root.
    this.content = this.attachShadow({ mode: 'open' })
    this.content.adoptedStyleSheets = [style]
  }

  async connectedCallback() {
    if (this.term) return
    const generation = ++this.generation
    const container = document.createElement('div')
    container.className = 'shell-job-terminal'
    container.setAttribute('role', 'region')
    container.setAttribute('aria-label', 'Interactive shell output')
    const controls = document.createElement('div')
    controls.className = 'shell-job-controls'
    this.status = document.createElement('span')
    this.status.className = 'shell-job-status'
    this.status.setAttribute('role', 'status')
    this.status.textContent = 'Connecting to job…'
    const interrupt = document.createElement('button')
    interrupt.textContent = 'Ctrl-C'
    interrupt.setAttribute('aria-label', 'Interrupt interactive shell')
    interrupt.onclick = () => this.send('interrupt')
    const stop = document.createElement('button')
    stop.textContent = 'Stop job'
    stop.onclick = () => this.send('stop')
    controls.append(interrupt, stop, this.status)
    this.content.replaceChildren(container, controls)
    // Historical tool rows shouldn't make every initial page load parse xterm.
    let Terminal: typeof import('@xterm/xterm').Terminal
    let FitAddon: typeof import('@xterm/addon-fit').FitAddon
    try {
      ;[{ Terminal }, { FitAddon }] = await Promise.all([import('@xterm/xterm'), import('@xterm/addon-fit')])
    } catch {
      if (generation === this.generation && this.status) this.status.textContent = 'Terminal could not load. Collapse and reopen to retry.'
      return
    }
    if (!this.isConnected || generation !== this.generation) return
    this.term = new Terminal({ convertEol: false, scrollback: 5000, fontSize: 12, cursorBlink: true, screenReaderMode: true })
    this.fit = new FitAddon()
    this.term.loadAddon(this.fit)
    this.term.open(container)
    this.term.onData(data => this.queueInput(data))
    this.unsubscribe = remoteStore.subscribeShellJobs(response => this.receive(response))
    this.unsubscribeState = remoteStore.subscribe(() => {
      const connected = remoteStore.getSnapshot().connection === 'connected'
      const reconnected = connected && !this.wasConnected
      this.wasConnected = connected
      if (!connected) {
        this.clearRetry()
        this.pending = null
        // An unacknowledged write may already have reached the PTY. Never
        // replay it (or later queued keystrokes) after reconnecting.
        if (this.pendingInput || this.inputQueue.length) {
          this.pendingInput = null
          this.inputQueue = []
          this.queuedInputBytes = 0
          if (this.status) this.status.textContent = 'Disconnected; unsent input discarded'
        } else if (this.status && !this.done) {
          this.status.textContent = 'Disconnected; reconnecting…'
        }
      } else if (reconnected && !this.done) {
        this.poll()
        this.resize()
      }
    })
    this.observer = new ResizeObserver(() => this.resize())
    this.observer.observe(container)
    this.wasConnected = remoteStore.getSnapshot().connection === 'connected'
    this.reset(this.getAttribute('job-id') ?? '')
  }

  disconnectedCallback() {
    this.generation++
    this.clearRetry()
    this.unsubscribe?.(); this.unsubscribe = null
    this.unsubscribeState?.(); this.unsubscribeState = null
    this.observer?.disconnect(); this.observer = null
    this.term?.dispose(); this.term = null; this.fit = null
    this.pending = null
    this.pendingInput = null
    this.inputQueue = []
    this.queuedInputBytes = 0
  }

  attributeChangedCallback(_name: string, oldValue: string | null, value: string | null) {
    if (oldValue !== value && this.term) this.reset(value ?? '')
  }

  private reset(job: string) {
    this.clearRetry()
    this.job = job; this.offset = 0; this.pending = null; this.done = false
    this.pendingInput = null; this.inputQueue = []; this.queuedInputBytes = 0
    this.term?.reset()
    this.poll()
    this.resize()
  }

  private send(action: 'output' | 'input' | 'resize' | 'interrupt' | 'stop', extra: { data?: string; rows?: number; cols?: number } = {}): string | null {
    if (!this.job) return null
    const requestId = crypto.randomUUID()
    return remoteStore.send({ type: 'shell_job', job_id: this.job, action, request_id: requestId, offset: this.offset, ...extra }) ? requestId : null
  }

  private queueInput(data: string) {
    if (!data || !this.wasConnected || this.done) return
    const encoder = new TextEncoder()
    const total = encoder.encode(data).length
    if (this.queuedInputBytes + total > 65536) {
      if (this.status) this.status.textContent = 'Input queue full; paste not sent'
      return
    }
    // Split paste at Unicode scalar boundaries to respect the protocol's byte
    // limit without splitting a multibyte UTF-8 character.
    let chunk = ''
    let bytes = 0
    const chunks: string[] = []
    for (const character of data) {
      const length = encoder.encode(character).length
      if (bytes + length > 8192) { chunks.push(chunk); chunk = ''; bytes = 0 }
      chunk += character; bytes += length
    }
    if (chunk) chunks.push(chunk)
    this.inputQueue.push(...chunks)
    this.queuedInputBytes += total
    this.flushInput()
  }

  private flushInput() {
    if (this.pendingInput || !this.wasConnected || !this.inputQueue.length) return
    const data = this.inputQueue[0]
    const id = this.send('input', { data })
    if (!id) return
    this.pendingInput = id
    this.inputQueue.shift()
    this.queuedInputBytes -= new TextEncoder().encode(data).length
  }

  private poll() {
    if (this.pending || this.retryTimer !== null || this.done || !this.isConnected || !this.wasConnected) return
    this.pending = this.send('output')
  }

  private clearRetry() {
    if (this.retryTimer !== null) window.clearTimeout(this.retryTimer)
    this.retryTimer = null
  }

  private resize() {
    if (!this.term || !this.fit || !this.isConnected) return
    try {
      this.fit.fit()
      this.send('resize', { rows: this.term.rows, cols: this.term.cols })
    } catch { /* A hidden or not-yet-laid-out terminal will be fitted later. */ }
  }

  private receive(response: ShellJobResponse) {
    if (response.jobId !== this.job) return
    if (response.action !== 'output') {
      if (response.action === 'input' && response.requestId === this.pendingInput) {
        this.pendingInput = null
        if (response.error) {
          // Preserve ordering: do not deliver later input after a failed write.
          this.inputQueue = []
          this.queuedInputBytes = 0
        } else {
          this.flushInput()
        }
      }
      if (response.error && this.status && this.status.textContent !== response.error) this.status.textContent = response.error
      return
    }
    if (response.requestId !== this.pending) return
    this.pending = null
    if (response.error) {
      if (this.status && this.status.textContent !== response.error) this.status.textContent = response.error
      if (response.retryable) {
        this.retryTimer = window.setTimeout(() => {
          this.retryTimer = null
          this.poll()
        }, 1000)
        return
      }
      this.done = true
      return
    }
    const page = response.result as ShellJobOutputPage | undefined
    if (!page || !Array.isArray(page.events)) return
    for (const event of page.events) {
      if (event.offset !== this.offset) {
        if (this.status) this.status.textContent = 'Output cursor mismatch; reopen the job.'
        this.done = true
        return
      }
      const raw = atob(event.dataBase64)
      this.term?.write(Uint8Array.from(raw, character => character.charCodeAt(0)))
      this.offset = event.nextOffset
    }
    // Recording can truncate while the process is still interactive. Keep
    // controls and completion monitoring alive even though output is lost.
    this.done = page.finished && this.offset >= page.totalBytes
    if (this.status) {
      const state = page.status === 'completed' ? 'Completed' : page.status === 'failed' ? 'Failed' : page.status === 'interrupted' ? 'Interrupted' : page.finished ? 'Job finished' : 'Running'
      const label = page.truncated ? `${state}; output truncated` : state
      // The live region reports state transitions, not every output poll.
      if (this.status.textContent !== label) this.status.textContent = label
    }
    if (!this.done) this.poll()
  }
}

if (!customElements.get('yeet-shell-job')) customElements.define('yeet-shell-job', YeetShellJob)
