<script setup lang="ts">
withDefaults(defineProps<{ size?: number; title?: string }>(), {
  size: 28,
  title: 'Yeet',
})

const stripePatternId = `yeet-mark-stripes-${Math.random().toString(36).slice(2, 9)}`
</script>

<template>
  <svg
    class="yeet-mark"
    :width="size"
    :height="size"
    viewBox="0 0 32 32"
    role="img"
    :aria-label="title"
    fill="none"
    stroke="currentColor"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <defs>
      <pattern :id="stripePatternId" width="32" height="2.5" patternUnits="userSpaceOnUse">
        <rect width="32" height="1.25" fill="currentColor" stroke="none" />
      </pattern>
    </defs>
    <g :fill="`url(#${stripePatternId})`" stroke="none">
      <path d="M2 13 14.5 22.75V31H2Z" />
      <path d="M30 13 17.5 22.75V31H30Z" />
      <path d="M16 1 26.5 11.25 16 20 5.5 11.25 16 1Z" />
    </g>
    <path d="M16 19.5v3.25M14.5 22.75 2 13M17.5 22.75 30 13" stroke="var(--bg, #080607)" stroke-width="1.5" />
  </svg>
</template>
