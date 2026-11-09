<template>
  <!-- BlueSlider's row, rebuilt to fit a flag between the label and the control; in a narrow
       panel the slider gives way to - / + steppers, one value per press. -->
  <div class="flex w-full justify-between items-center">
    <div
      class="min-w-0 max-w-[45%] shrink-0 flex items-center mr-6"
      style="min-width: 130px"
    >
      <span
        class="truncate text-start text-white"
        :class="disabled ? 'opacity-30' : ''"
      >{{ label }}</span>
      <slot name="flag" />
    </div>
    <div
      v-if="compact"
      class="flex items-center gap-1"
    >
      <BlueButton
        variant="icon"
        icon="mdi-minus"
        density="compact"
        theme="dark"
        :tooltip="`Lower ${label}`"
        :disabled="disabled || index <= 0"
        @click="emit('update', index - 1)"
      />
      <span
        class="stepper-value"
        :class="disabled ? 'opacity-30' : ''"
        role="status"
        :aria-label="label"
      >{{ items[index] }}</span>
      <BlueButton
        variant="icon"
        icon="mdi-plus"
        density="compact"
        theme="dark"
        :tooltip="`Raise ${label}`"
        :disabled="disabled || index >= items.length - 1"
        @click="emit('update', index + 1)"
      />
    </div>
    <div
      v-else
      :ref="nameSlider"
      class="flex basis-[max(50%,320px)] min-w-[140px] items-center"
    >
      <BlueSlider
        class="pro-list-slider"
        :model-value="index"
        :disabled="disabled"
        :name="name"
        :min="0"
        :max="items.length - 1"
        :step="1"
        :keyboard-step-multiplier-limit="1"
        :format-display="(position: number) => items[Math.round(position)]"
        :label-min="items[0]"
        :label-max="items[items.length - 1]"
        theme="dark"
        value-weight="regular"
        @dblclick.capture.stop="noEdit"
        @update:model-value="emit('update', Math.round($event as number))"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { BlueButton, BlueSlider } from '@bluerobotics/bluevue'

const props = defineProps<{
  label: string
  name: string
  /** The values, as shown. */
  items: string[]
  /** Position of the current value in `items`. */
  index: number
  disabled: boolean
  /** Steppers instead of a slider, for a narrow panel. */
  compact: boolean
}>()

const emit = defineEmits<{
  /** A new position in `items`. */
  (event: 'update', index: number): void
}>()

/**
 * The slider moves by position, so BlueSlider's double-click editor would ask for a position,
 * not a value: it stays shut.
 */
const noEdit = () => {}

/** BlueSlider names its range input only through its own label, which this row replaces. */
const nameSlider = (column: unknown) => {
  if (column instanceof HTMLElement) {
    column.querySelector('input[type="range"]')?.setAttribute('aria-label', props.label)
  }
}
</script>

<style scoped>
/* The track fills the column the row gives it, as in BlueSlider's own row. */
:deep(.pro-list-slider > div) {
  flex-basis: 100%;
}
.stepper-value {
  min-width: 5.5em;
  text-align: center;
  font-variant-numeric: tabular-nums;
}
</style>
