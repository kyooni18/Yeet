<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { useModalFocus } from '@/composables/useModalFocus'
import { useRemoteStore } from '@/stores/remote'
import ModelPicker from './ModelPicker.vue'

const remote = useRemoteStore()
const router = useRouter()
const reasoning = ['auto', 'low', 'medium', 'high']
const permissionLabel = computed(() => remote.permissionMode === 'unlimited' ? 'Unlimited' : remote.permissionMode === 'auto' ? 'Auto approve' : remote.permissionMode === 'ask' ? 'Ask first' : 'Unknown')
const skills = computed(() => remote.state.available_capabilities.filter((capability) => capability.kind === 'skill'))
const attachedSkillCount = computed(() => skills.value.filter((skill) => skill.enabled).length)
const sessionControlsReadOnly = computed(() => remote.connection !== 'connected')

function close() {
  remote.mobileStatusOpen = false
}

const { modalRoot: panel, handleModalKeydown: handleKeydown } = useModalFocus(
  () => remote.mobileStatusOpen,
  close,
)

function openSettings(section = 'runtime') {
  close()
  void router.push(`/settings/${section}`)
}
</script>

<template>
  <div v-if="remote.mobileStatusOpen" class="session-controls-layer" data-testid="status-sheet" @click.self="close">
    <section
      ref="panel"
      class="session-controls-panel"
      data-testid="session-controls"
      aria-modal="true"
      role="dialog"
      aria-labelledby="session-controls-title"
      tabindex="-1"
      @keydown="handleKeydown"
    >
      <div class="session-controls-grip" aria-hidden="true"></div>
      <div class="session-controls-heading">
        <div><span>Current session</span><h2 id="session-controls-title">Session settings</h2></div>
        <button class="icon-button" aria-label="Close session controls" @click="close">×</button>
      </div>
      <div class="session-control-summary">
        <span><i class="status-dot" :class="`status-${remote.connection}`"></i>{{ remote.connection }}</span>
        <span>{{ remote.contextPercent == null ? 'Context —' : `${remote.contextPercent}% context` }}</span>
        <button type="button" @click="openSettings('sandbox')">{{ permissionLabel }} →</button>
      </div>
      <p v-if="sessionControlsReadOnly" class="session-controls-readonly" role="status">Session controls are read-only while Yeet Remote reconnects.</p>
      <fieldset class="session-control-scope" :disabled="sessionControlsReadOnly">
      <div class="session-control-field">
        <div class="session-control-label-row"><span>Model</span><small v-if="remote.state.is_streaming">Next response</small></div>
        <ModelPicker variant="panel" />
      </div>
      <div class="session-control-field">
        <div class="session-control-label-row"><span>Reasoning</span><small v-if="remote.state.is_streaming">Next response</small></div>
        <div class="segmented-control">
          <button
            v-for="level in reasoning"
            :key="level"
            :class="{ active: remote.state.active_reasoning_level === level }"
            :aria-pressed="remote.state.active_reasoning_level === level"
            @click="remote.selectReasoning(level)"
          >{{ level }}</button>
        </div>
      </div>
      <div class="session-control-field">
        <div class="session-control-label-row"><span>Goal</span><small>{{ remote.state.goal_mode ? 'Strict success judging' : 'Normal completion' }}</small></div>
        <div class="segmented-control">
          <button :class="{ active: remote.state.goal_mode }" :aria-pressed="remote.state.goal_mode" @click="remote.setGoal(true)">ON</button>
          <button :class="{ active: !remote.state.goal_mode }" :aria-pressed="!remote.state.goal_mode" @click="remote.setGoal(false)">OFF</button>
        </div>
      </div>
      <div v-if="skills.length" class="session-control-field skill-control-field">
        <div class="session-control-label-row">
          <span>Skills</span>
          <small>{{ attachedSkillCount ? `${attachedSkillCount} attached` : 'Attach to this session' }}</small>
        </div>
        <div class="session-skill-list" aria-label="Session skills">
          <button
            v-for="skill in skills"
            :key="skill.id"
            class="session-skill-chip"
            :class="{ attached: skill.enabled }"
            type="button"
            :disabled="remote.state.is_streaming"
            :aria-pressed="skill.enabled"
            :aria-label="`${skill.enabled ? 'Detach' : 'Attach'} ${skill.name} skill`"
            @click="remote.toggleCapability(skill.id)"
          >
            <span class="skill-status-mark" aria-hidden="true">{{ skill.enabled ? '✓' : '+' }}</span>
            <span class="skill-chip-copy"><strong>{{ skill.name }}</strong><small>{{ skill.enabled ? 'Attached' : 'Attach' }}</small></span>
          </button>
        </div>
      </div>
      </fieldset>
      <div class="session-controls-footer">
        <span>Model, reasoning, Goal mode, skills, and permission state for this session.</span>
        <button class="secondary-button" @click="openSettings()">All settings</button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.session-controls-layer { position: fixed; top: var(--visual-viewport-top); right: 0; left: 0; z-index: 160; display: grid; height: var(--visual-viewport-height); place-items: center; padding: 20px; background: rgba(0,0,0,.48); backdrop-filter: blur(4px); -webkit-backdrop-filter: blur(4px); }
.session-controls-panel { width: min(420px,calc(100vw - 40px)); max-height: min(680px,calc(var(--visual-viewport-height) - 40px)); overflow-y: auto; overscroll-behavior: contain; padding: 14px; border: 1px solid var(--border-strong); border-radius: 18px; background: rgba(15,10,12,.975); box-shadow: 0 24px 70px rgba(0,0,0,.4); }
.session-controls-grip { display: none; }
.session-controls-heading { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 2px 2px 12px; }
.session-controls-heading > div { min-width: 0; }
.session-controls-heading > div > span { color: var(--muted); font-size: 9px; text-transform: uppercase; letter-spacing: .08em; }
.session-controls-heading h2 { margin: 2px 0 0; font-size: 17px; font-weight: 650; }
.session-control-summary { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin-bottom: 12px; }
.session-control-summary > span,.session-control-summary > button { display: inline-flex; min-height: 28px; align-items: center; gap: 6px; padding: 0 8px; border: 1px solid var(--border); border-radius: 999px; background: transparent; color: var(--muted); font-size: 10px; }
.session-control-summary > button { cursor: pointer; }
.session-control-summary > button:hover { border-color: var(--border-strong); color: var(--text); }
.session-control-scope { min-width: 0; margin: 0; padding: 0; border: 0; }
.session-controls-readonly { margin: 0 0 10px; padding: 8px 10px; border: 1px solid rgba(255,119,82,.14); border-radius: 10px; background: rgba(255,91,62,.045); color: var(--muted); font-size: 10px; line-height: 1.4; }
.session-control-field { display: grid; gap: 7px; padding: 12px 0; border-top: 1px solid var(--border); }
.session-control-field > span,.session-control-label-row > span { color: var(--muted); font-size: 10px; font-weight: 600; }
.session-control-field .segmented-control { width: 100%; }
.session-control-field .segmented-control button { min-width: 0; flex: 1; }
.session-control-label-row { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.session-control-label-row small { color: var(--dim); font-size: 9.5px; }
.session-skill-list { display: flex; gap: 7px; overflow-x: auto; overscroll-behavior-x: contain; padding: 1px 1px 3px; scrollbar-width: none; }
.session-skill-list::-webkit-scrollbar { display: none; }
.session-skill-chip { display: grid; min-width: 138px; min-height: 48px; grid-template-columns: 26px minmax(0,1fr); align-items: center; gap: 8px; padding: 7px 9px; border: 1px solid var(--border); border-radius: 13px; background: rgba(255,255,255,.025); color: var(--soft); text-align: left; cursor: pointer; }
.session-skill-chip:hover { border-color: var(--border-strong); background: rgba(255,255,255,.045); }
.session-skill-chip.attached { border-color: rgba(255,119,82,.24); background: rgba(255,91,62,.085); color: var(--text); }
.session-skill-chip:disabled { opacity: .5; cursor: default; }
.skill-status-mark { display: grid; width: 26px; height: 26px; place-items: center; border-radius: 9px; background: rgba(255,255,255,.06); color: var(--muted); font-size: 14px; }
.session-skill-chip.attached .skill-status-mark { background: rgba(255,103,70,.14); color: var(--brand-hot); }
.skill-chip-copy { display: flex; min-width: 0; flex-direction: column; gap: 2px; }
.skill-chip-copy strong { overflow: hidden; font-size: 11px; font-weight: 620; text-overflow: ellipsis; white-space: nowrap; }
.skill-chip-copy small { color: var(--dim); font-size: 9px; }
.session-controls-footer { display: flex; align-items: center; justify-content: space-between; gap: 14px; padding-top: 12px; border-top: 1px solid var(--border); }
.session-controls-footer > span { max-width: 240px; color: var(--dim); font-size: 9.5px; line-height: 1.35; }
.session-controls-footer .secondary-button { min-height: 34px; flex: 0 0 auto; font-size: 10.5px; }
@media (hover: none) and (pointer: coarse) and (min-width: 900px) {
  .session-controls-heading .icon-button { width: 44px; height: 44px; }
  .session-control-summary > button { min-height: 44px; }
  .session-control-field .segmented-control button,
  .session-controls-footer .secondary-button { min-height: 44px; }
  :deep(.model-picker-search) { min-height: 44px !important; font-size: 16px; }
  :deep(.model-picker-providers button) { min-width: 44px; min-height: 44px; }
}

@media (max-width:899px) {
  .session-controls-layer { align-items: end; padding: max(12px,env(safe-area-inset-top)) max(10px,env(safe-area-inset-right)) max(10px,env(safe-area-inset-bottom)) max(10px,env(safe-area-inset-left)); }
  .session-controls-panel { width: min(100%,520px); max-height: min(74dvh,calc(var(--visual-viewport-height) - 24px)); padding: 7px 14px 14px; border-radius: 22px; box-shadow: 0 20px 55px rgba(0,0,0,.34); }
  .session-controls-grip { display: block; width: 34px; height: 4px; margin: 2px auto 10px; border-radius: 99px; background: var(--border-strong); }
  .session-controls-heading { padding-bottom: 10px; }
  .session-controls-heading h2 { font-size: 16px; }
  .session-controls-heading > div > span { font-size: 8.5px; }
  .session-control-summary { gap: 5px; margin-bottom: 10px; }
  .session-control-summary > span,.session-control-summary > button { min-height: 32px; padding-right: 8px; padding-left: 8px; font-size: 9.5px; }
  .session-controls-heading .icon-button { width: 44px; height: 44px; }
  .session-control-summary > button { min-height: 44px; }
  .session-control-field { gap: 6px; padding-top: 10px; padding-bottom: 10px; }
  .session-control-field > span,.session-control-label-row > span { font-size: 9.5px; }
  .session-control-label-row small { font-size: 9px; }
  .session-control-field .segmented-control button,.session-controls-footer .secondary-button { min-height: 44px; }
  .session-skill-list { gap: 6px; }
  .session-skill-chip { min-width: min(42vw,164px); min-height: 48px; grid-template-columns: 26px minmax(0,1fr); padding: 6px 8px; border-radius: 12px; }
  .skill-status-mark { width: 26px; height: 26px; border-radius: 8px; font-size: 13px; }
  .skill-chip-copy strong { font-size: 10.5px; }
  .skill-chip-copy small { font-size: 8.5px; }
  .session-controls-footer { gap: 12px; padding-top: 10px; }
  .session-controls-footer > span { font-size: 9px; }
}
</style>
