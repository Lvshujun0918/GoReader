<script setup lang="ts">
import { DialogRoot, type DialogRootEmits, type DialogRootProps } from 'reka-ui'

const props = defineProps<DialogRootProps>()
const emits = defineEmits<DialogRootEmits>()
</script>

<template>
  <DialogRoot v-bind="props" @update:open="(v: boolean) => emits('update:open', v)">
    <slot />
  </DialogRoot>
</template>
