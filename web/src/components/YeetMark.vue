<script setup lang="ts">
withDefaults(defineProps<{ size?: number; title?: string }>(), {
  size: 28,
  title: 'Yeet',
})
</script>

<template>
  <svg
    class="yeet-mark"
    :width="size"
    :height="size"
    viewBox="0 0 32 32"
    role="img"
    :aria-label="title"
  >
    <g fill="currentColor">
      <rect x="15" y="2" width="2" height="2" rx="1" />
      <rect x="13" y="5" width="6" height="2" rx="1" />
      <rect x="11" y="8" width="10" height="2" rx="1" />
      <rect x="7" y="12" width="7" height="2" rx="1" />
      <rect x="15" y="12" width="2" height="2" rx="1" />
      <rect x="18" y="12" width="7" height="2" rx="1" />
      <rect x="5" y="16" width="10" height="2" rx="1" />
      <rect x="17" y="16" width="10" height="2" rx="1" />
      <rect x="3" y="20" width="12" height="2" rx="1" />
      <rect x="17" y="20" width="12" height="2" rx="1" />
      <rect x="2" y="24" width="13" height="2" rx="1" />
      <rect x="17" y="24" width="13" height="2" rx="1" />
      <rect x="1" y="28" width="14" height="2" rx="1" />
      <rect x="17" y="28" width="14" height="2" rx="1" />
    </g>
  </svg>
</template>
