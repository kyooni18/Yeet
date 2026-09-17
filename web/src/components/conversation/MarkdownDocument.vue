<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import DOMPurify from 'dompurify'
import MarkdownIt from 'markdown-it'
import { copyTextToClipboard } from '@/utils/clipboard'

const props = defineProps<{ content: string }>()
const root = ref<HTMLElement | null>(null)
let observer: IntersectionObserver | null = null
let disposed = false

let copyFeedbackSequence = 0

const markdown = new MarkdownIt({
  html: false,
  linkify: true,
  typographer: true,
  breaks: false,
})

markdown.renderer.rules.link_open = (tokens, index, options, env, self) => {
  tokens[index].attrSet('target', '_blank')
  tokens[index].attrSet('rel', 'noreferrer noopener')
  return self.renderToken(tokens, index, options)
}

markdown.renderer.rules.fence = (tokens, index) => {
  const token = tokens[index]
  const language = token.info.trim().split(/\s+/)[0] || 'text'
  const code = markdown.utils.escapeHtml(token.content)
  const safeLanguage = markdown.utils.escapeHtml(language)
  return `<figure class="code-block"><figcaption><span>${safeLanguage}</span><button type="button" data-copy-code aria-label="Copy ${safeLanguage} code" aria-live="polite">Copy</button></figcaption><pre tabindex="0" role="group" aria-label="${safeLanguage} code block"><code class="language-${safeLanguage}" data-language="${safeLanguage}">${code}</code></pre></figure>`
}

const html = computed(() => DOMPurify.sanitize(markdown.render(props.content || ''), {
  ADD_ATTR: ['data-copy-code', 'data-language', 'target', 'rel'],
}))

async function highlight(code: HTMLElement) {
  if (code.dataset.highlighted === 'yes') return
  const source = code.textContent ?? ''
  if (source.length > 20_000) {
    // Highlight.js can monopolize the main thread on generated logs/large source
    // dumps. Keep those blocks plain and immediately interactive instead.
    code.dataset.highlighted = 'yes'
    return
  }
  try {
    const module = await import('highlight.js/lib/common')
    if (!disposed) module.default.highlightElement(code)
  } catch {
    // Plain code remains fully readable when the optional highlighter cannot load.
  }
}

function observeCodeBlocks() {
  observer?.disconnect()
  observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue
      observer?.unobserve(entry.target)
      void highlight(entry.target as HTMLElement)
    }
  }, { rootMargin: '240px' })
  root.value?.querySelectorAll<HTMLElement>('pre code').forEach((code) => observer?.observe(code))
}

function handleClick(event: MouseEvent) {
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-copy-code]')
  if (!button || !root.value?.contains(button)) return
  const code = button.closest('.code-block')?.querySelector('code')?.textContent ?? ''
  const original = button.dataset.copyOriginal || button.textContent || 'Copy'
  const feedbackId = String(++copyFeedbackSequence)
  button.dataset.copyOriginal = original
  button.dataset.copyFeedback = feedbackId
  void copyTextToClipboard(code).then((copied) => {
    if (!button.isConnected || button.dataset.copyFeedback !== feedbackId) return
    const feedback = copied ? 'Copied' : 'Copy failed'
    button.textContent = feedback
    window.setTimeout(() => {
      if (!button.isConnected || button.dataset.copyFeedback !== feedbackId) return
      button.textContent = original
      delete button.dataset.copyFeedback
      delete button.dataset.copyOriginal
    }, 1500)
  })
}

watch(html, () => void nextTick(observeCodeBlocks))
onMounted(observeCodeBlocks)
onBeforeUnmount(() => {
  disposed = true
  observer?.disconnect()
})
</script>

<template>
  <div ref="root" class="markdown-document" @click="handleClick" v-html="html"></div>
</template>
