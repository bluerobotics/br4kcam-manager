<template>
  <Teleport to="body">
    <Transition
      name="copy-toast-fade"
      @enter="(element) => (element as HTMLElement).showPopover()"
    >
      <div
        v-if="message"
        class="copy-feedback-toast"
        popover="manual"
        role="status"
        aria-live="polite"
        @click="emit('dismiss')"
      >
        <div class="copy-feedback-toast__card">
          {{ message }}
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
defineProps<{
  message: string | null
}>()

const emit = defineEmits<{
  (e: 'dismiss'): void
}>()
</script>

<style scoped>
.copy-feedback-toast {
  /* Popover top layer: stacks above a modal dialog's blurred ::backdrop. Reset UA popover styles. */
  inset: auto;
  margin: 0;
  padding: 0;
  border: 0;
  background: transparent;
  color: inherit;
  overflow: visible;
  position: fixed;
  bottom: 1.25rem;
  left: 50%;
  z-index: 10000;
  max-width: min(92vw, 420px);
  transform: translateX(-50%);
  cursor: pointer;
}

.copy-feedback-toast__card {
  padding: 0.6rem 1rem;
  border-radius: 0.5rem;
  border: 1px solid rgb(255 255 255 / 0.18);
  background: rgb(32 32 32 / 0.72);
  color: #fff;
  font-size: 0.875rem;
  line-height: 1.35;
  text-align: center;
  backdrop-filter: blur(6px);
  box-shadow: 0 4px 16px rgb(0 0 0 / 0.35);
  word-break: break-word;
}

.copy-toast-fade-enter-active,
.copy-toast-fade-leave-active {
  transition: opacity 0.15s ease;
}

.copy-toast-fade-enter-from,
.copy-toast-fade-leave-to {
  opacity: 0;
}

@media (prefers-reduced-motion: reduce) {
  .copy-toast-fade-enter-active,
  .copy-toast-fade-leave-active {
    transition: none;
  }
}
</style>
