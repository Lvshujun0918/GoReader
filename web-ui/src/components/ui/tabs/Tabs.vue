<script setup lang="ts">
import { TabsRoot, type TabsRootEmits, type TabsRootProps } from 'reka-ui'

const props = defineProps<TabsRootProps>()
const emits = defineEmits<TabsRootEmits>()
</script>

<template>
  <TabsRoot
    v-bind="props"
    @update:model-value="(v: string | number) => emits('update:modelValue', v)"
  >
    <slot />
  </TabsRoot>
</template>
