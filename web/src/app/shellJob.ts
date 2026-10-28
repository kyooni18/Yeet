import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import { remoteStore } from '@/store/remoteStore'
import type { ShellJobOutputPage, ShellJobResponse } from '@/remote/protocol'

/** A viewer for an existing approved job; it never launches a command. */
class YeetShellJob extends HTMLElement {
  static observedAttributes = ['job-id']
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
  private retryTimer: number | null = null

  connectedCallback() {
    if (this.term) return
    this.style.display = 'block'
    this.style.minWidth = '0'
    this.style.width = '100%'
    const container = document.createElement('div')
    container.style.cssText = 'height:220px;min-width:0;overflow:hidden;background:#111;padding:6px'
    const controls = document.createElement('div')
    controls.style.cssText = 'display:flex;align-items:center;gap:8px;padding:4px 0'
    this.status = document.createElement('span')
    this.status.textContent = 'Connecting to job…'
    const interrupt = document.createElement('button')
    interrupt.textContent = 'Ctrl-C'
    interrupt.setAttribute('aria-label', 'Interrupt interactive shell')
    interrupt.onclick = () => this.send('interrupt')
    const stop = document.createElement('button')
    stop.textContent = 'Stop job'
    stop.onclick = () => this.send('stop')
    controls.append(interrupt, stop, this.status)
    this.replaceChildren(container, controls)
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
      if (response.error && this.status) this.status.textContent = response.error
      return
    }
    if (response.requestId !== this.pending) return
    this.pending = null
    if (response.error) {
      if (this.status) this.status.textContent = response.error
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
      this.status.textContent = page.truncated ? `${state}; output truncated` : state
    }
    if (!this.done) this.poll()
  }
}

if (!customElements.get('yeet-shell-job')) customElements.define('yeet-shell-job', YeetShellJob)
