<template>
  <BlueDialog
    :model-value="props.show"
    :persistent="props.persistent"
    :width="props.width"
    class="glass-dialog"
    header-class="px-5 py-10"
    footer-class="px-5 py-[14px]"
    body-class="px-6 pt-4 pb-[17px] w-full bg-black/[0.169] bluevue-inset-1"
    @update:model-value="(value: boolean) => { if (!value) emit('dismiss') }"
  >
    <template
      v-if="props.logo"
      #header
    >
      <img
        src="../../public/assets/logo.svg"
        class="mx-auto h-auto w-[188px] shrink-0"
        alt=""
      >
    </template>
    <h2 class="mb-4 text-center text-lg text-white">
      {{ props.title }}
    </h2>
    <slot />
    <template
      v-if="$slots.actions"
      #footer
    >
      <slot name="actions" />
    </template>
  </BlueDialog>
</template>

<script setup lang="ts">
import { BlueDialog } from '@bluerobotics/bluevue'

// Defaults must go through withDefaults: Vue casts an absent boolean prop to false,
// so an omitted `logo` or `persistent` would otherwise read as an explicit false.
const props = withDefaults(
  defineProps<{
    show: boolean
    title: string
    width?: string
    /** False lets a click outside or Esc close the dialog. */
    persistent?: boolean
    /** False drops the logo, for compact confirmation dialogs. */
    logo?: boolean
  }>(),
  {
    width: '400px',
    persistent: true,
    logo: true,
  },
)

const emit = defineEmits<{
  (e: 'dismiss'): void
}>()
</script>
