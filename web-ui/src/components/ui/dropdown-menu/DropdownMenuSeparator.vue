<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { DropdownMenuSeparator as Primitive } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <Primitive
    :class="cn('-mx-1 my-1 h-px bg-border', props.class)"
  />
</template>
