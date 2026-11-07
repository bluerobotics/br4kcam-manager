<template>
  <BlueSelect
    :items="items"
    @update:model-value="pick"
  />
</template>

<script setup lang="ts">
import { BlueSelect } from '@bluerobotics/bluevue'

// bluevue's BlueSelect emits `value || name`, so an item worth 0 comes back as its name: map it back.
const props = defineProps<{
  items: { name: string; value?: unknown }[]
}>()

const emit = defineEmits<{
  (event: 'update:modelValue', value: unknown): void
}>()

const pick = (picked: unknown) => {
  const named = props.items.some((item) => item.value === picked)
    ? undefined
    : props.items.find((item) => item.name === picked && item.value !== undefined)
  emit('update:modelValue', named ? named.value : picked)
}
</script>
