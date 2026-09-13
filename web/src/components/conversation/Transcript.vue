<script setup lang="ts">
import { nextTick, onMounted, ref, watch } from 'vue'
import { useRemoteStore } from '@/stores/remote'
import YeetMark from '@/components/YeetMark.vue'
import ConversationEntry from './ConversationEntry.vue'

const remote = useRemoteStore()
const scroller = ref<HTMLElement | null>(null)
const following = ref(true)

function nearBottom() {
  const element = scroller.value
  if (!element) return true
  return element.scrollHeight - element.scrollTop - element.clientHeight < 160
}

function handleScroll() {
  following.value = nearBottom()
}

function scrollLatest(behavior: ScrollBehavior = 'smooth') {
  const element = scroller.value
  if (!element) return
  following.value = true
  if (!remote.entries.length) {
    element.scrollTo({ top: 0, behavior })
    return
  }
  element.scrollTo({ top: element.scrollHeight, behavior })
}

watch(
  () => [remote.state.conversation_revision, remote.state.active_assistant_text, remote.state.active_reasoning_text, remote.entries.length],
  () => {
    if (!following.value) return
    void nextTick(() => scrollLatest('auto'))
  },
)

onMounted(() => void nextTick(() => scrollLatest('auto')))
</script>

<template>
  <main ref="scroller" class="transcript-scroller" data-testid="transcript" @scroll.passive="handleScroll">
    <div class="transcript">
      <div v-if="!remote.entries.length" class="empty-transcript">
        <div class="empty-mark"><YeetMark :size="54" /></div>
        <h1>What can Yeet do for you?</h1>
        <p>Ask it to build, investigate, use tools, inspect files, or continue where you left off.</p>
      </div>
      <ConversationEntry v-for="entry in remote.entries" :key="entry.id" :entry="entry" />
    </div>
    <button v-if="!following" class="jump-latest" @click="scrollLatest()">↓ Latest</button>
  </main>
</template>
