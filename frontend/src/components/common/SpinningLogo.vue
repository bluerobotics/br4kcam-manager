<template>
  <div class="flex flex-col items-center justify-between p-4">
    <img
      class="lds-dual-ring aspect-square"
      :style="{ width: size }"
      src="@/assets/common/blue-robotics-logo.svg"
      alt=""
    >
    <span class="text-base">{{ subtitle }}</span>
  </div>
</template>

<script lang="ts" setup>
import { defineProps } from 'vue'

withDefaults(defineProps<{
  size?: string
  subtitle?: string
}>(), {
  size: '10%',
  subtitle: '...',
});

</script>

<style scoped>
.lds-dual-ring {
  display: block;
  margin-top: 5%;
  margin-bottom: 2%;
  animation-name: rotation360;
  animation-duration: 2s;
  animation-iteration-count: infinite;
}

@keyframes rotation360 {
  0% {
    transform: rotate(0deg);
  }

  100% {
    transform: rotate(360deg);
  }
}
</style>
